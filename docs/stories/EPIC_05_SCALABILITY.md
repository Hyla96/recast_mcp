# Epic 05: Scalability

**Product:** Dynamic MCP Server Builder
**Date:** 2026-03-28
**Status:** Draft
**Epic Owner:** Distributed Systems Lead
**Target:** Scale from 500 servers (Stage 1) to 100,000+ servers (Stage 3) with 99.99% uptime, <200ms p95 gateway overhead, and multi-region deployment across US, EU, and APAC.

---

## Context

This epic covers the infrastructure and architecture evolution required to scale the Dynamic MCP Server Builder from a single-instance Railway deployment to a globally distributed, multi-region EKS platform. Stories are sequenced to follow the infrastructure evolution path defined in the PRD: Stage 1 (0-500 servers, Railway) to Stage 2 (500-10K, ECS Fargate) to Stage 3 (10K-100K+, EKS with Aurora Global).

The gateway uses a multiplexed architecture -- a single Rust process (axum + tokio) serves all MCP server configs from memory. Scaling means adding more gateway instances behind a load balancer, not running one container per MCP server. All stories assume this multiplexed model is preserved.

**Key architectural constraints:**
- Gateway must remain stateless (all state in PostgreSQL + cache)
- Credential injector sidecar runs as a separate process; gateway never holds raw credentials
- Config changes propagate via PostgreSQL LISTEN/NOTIFY (Stage 1-2) or SNS/SQS (Stage 3)
- MCP protocol requires persistent connections (SSE/Streamable HTTP); serverless is not viable for the serving path

---

## Stories

---

### S-070: Stateless Gateway Design Verification

**Priority:** P0
**Estimated Effort:** 3 story points
**Stage:** Pre-Stage 2 (must complete before horizontal scaling)

#### Description

Verify and enforce that all gateway instances are fully stateless. Every piece of mutable state must reside in PostgreSQL (source of truth) or in an in-memory cache that is fully rebuildable from PostgreSQL on startup. No local disk state, no sticky sessions, no instance-specific data. This is the foundational contract that enables horizontal scaling -- if any instance can die and be replaced without data loss or service disruption, horizontal scaling works. If not, every subsequent story in this epic is built on a broken assumption.

Produce a statelessness contract document that explicitly enumerates what state exists, where it lives, and how it is rebuilt.

#### Acceptance Criteria

1. Audit all gateway code paths and enumerate every piece of in-process state (caches, counters, connection tracking, rate limit buckets, config maps).
2. For each piece of state, classify it as: (a) rebuildable from PostgreSQL, (b) ephemeral and loss-tolerant, or (c) stateful and must be externalized. Category (c) items must be zero.
3. Verify that killing a gateway instance mid-request results in zero data loss. Clients reconnect to another instance and resume operation. Write a test that validates this.
4. Verify that a freshly started gateway instance reaches full operational state (all configs loaded, cache warm) within 60 seconds for up to 10,000 server configs.
5. Confirm no use of local filesystem for state (no SQLite, no file-based caches, no temp files that outlive a single request).
6. Confirm no sticky session requirements in any load balancer configuration. Any instance can serve any MCP server request.
7. Produce a `STATELESSNESS_CONTRACT.md` document listing: all in-process state, its category, rebuild mechanism, and time-to-rebuild. This document becomes a design review artifact for all future gateway changes.
8. Add a CI lint rule or architecture decision record (ADR) that flags any introduction of local persistent state (file writes outside `/tmp`, SQLite usage, etc.).

#### Technical Notes

- The in-memory config cache (loaded from PostgreSQL on startup, updated via LISTEN/NOTIFY) is the primary state to validate. It must be fully rebuildable.
- Rate limiting counters stored in-process are loss-tolerant at Stage 1 (single instance). At Stage 2+, they must move to Redis (see S-074). Flag this as a known migration.
- Connection state for active SSE/Streamable HTTP sessions is inherently ephemeral -- clients must handle reconnection. Verify the MCP client reconnection contract.
- The credential injector sidecar is a separate process. Its statelessness must be verified independently.

#### Dependencies

- None (this is a prerequisite for all other stories in this epic)

---

### S-071: Database Connection Pooling and Optimization

**Priority:** P0
**Estimated Effort:** 5 story points
**Stage:** Stage 2 (ECS Fargate migration)

#### Description

Implement robust database connection pooling and query optimization for the hot path. At Stage 2 with 4-8 Fargate tasks, each opening connections to PostgreSQL, connection exhaustion becomes a real risk (RDS db.t4g.medium supports ~100 connections by default). The gateway's hot paths -- config lookup on every MCP request and audit log writes on every credential access -- must be optimized for throughput.

Implement connection pooling (PgBouncer as a sidecar or sqlx's built-in pool), prepare the read/write splitting abstraction, optimize queries on critical paths, and instrument connection pool metrics.

#### Acceptance Criteria

1. Connection pool configured with: min 5, max 20 connections per gateway instance. Pool size tunable via environment variable without code change.
2. Connection pool metrics exported: active connections, idle connections, wait queue depth, connection acquisition latency (p50/p95/p99), checkout timeouts.
3. Read/write connection split abstraction in place. All read queries go through a `read_pool()` accessor, all writes through `write_pool()`. At Stage 2 both point to the same database. At Stage 3 (Aurora), `read_pool()` points to the regional read replica with zero code changes.
4. Config lookup query (`SELECT config_json FROM mcp_servers WHERE slug = $1 AND status = 'active'`) executes in <1ms at p95 with an appropriate index. Verified via `EXPLAIN ANALYZE` on a dataset of 10,000 server configs.
5. Audit log writes use batched inserts (flush every 100ms or 50 records, whichever comes first) to reduce per-request write overhead. No audit event is lost -- buffer is flushed on graceful shutdown.
6. Prepared statements used for the top 5 most frequent queries (config lookup by slug, config lookup by server_id, audit log insert, credential lookup by server_id, request log insert).
7. Connection health checks enabled. Stale connections are evicted. Pool recovers automatically from transient database failures (connection reset, DNS change during RDS failover) within 30 seconds.
8. Load test: 8 gateway instances, 10,000 server configs, 5,000 requests/second sustained for 10 minutes. Zero connection pool exhaustion errors. p95 query latency <5ms for config lookups.

#### Technical Notes

- **sqlx pool vs PgBouncer:** sqlx's built-in pool (already a dependency) supports min/max connections, health checks, and prepared statements. PgBouncer adds connection multiplexing (many application connections mapped to fewer database connections) which is valuable when pod count exceeds database connection limits. At Stage 2 (8 tasks x 20 max = 160 connections vs ~100 limit), PgBouncer is needed. Deploy PgBouncer as a sidecar container in each ECS task, or use RDS Proxy ($18/month).
- **Audit log batching:** Use a bounded channel (tokio mpsc, capacity 10,000) with a background flush task. On graceful shutdown (SIGTERM), drain the channel before exiting.
- **Index strategy:** Ensure `mcp_servers(slug)` has a unique index, `mcp_servers(user_id)` has a btree index, `audit_log(server_id, created_at)` has a composite index, `request_logs(server_id, created_at)` has a composite index.
- At Stage 3, RDS Proxy becomes the connection pooler and PgBouncer can be removed.

#### Dependencies

- S-070 (stateless gateway verification -- confirms no local state conflicts with pooling)

---

### S-072: Config Sharding Strategy

**Priority:** P1
**Estimated Effort:** 8 story points
**Stage:** Stage 2 (required before scaling past 4 instances)

#### Description

Design and implement the strategy for distributing MCP server configs across multiple gateway instances. At Stage 1 (single instance), every config lives in one process. At Stage 2+ with multiple instances, we must decide: does every instance hold every config (full replication), or does each instance hold a subset (sharding)?

This story evaluates three approaches, benchmarks them, and implements the chosen strategy:

- **Option A: Full replication.** Every instance loads all configs into memory. Any instance can serve any request. Load balancer uses round-robin. Simple but memory-heavy at scale (100K configs x ~1KB each = ~100MB per instance -- acceptable).
- **Option B: Consistent hashing by server_id.** Each instance owns a hash range. Load balancer routes by server slug hash. Lower per-instance memory but requires hash-aware routing and rebalancing on scale events.
- **Option C: Partition by user_id.** Group all of a user's servers on one instance. Simplifies per-user rate limiting and reduces cache churn. Uneven distribution risk (power users with thousands of servers).

#### Acceptance Criteria

1. Benchmark all three options at 10K, 50K, and 100K server configs. Measure per-instance: memory usage, config load time (cold start), cache invalidation latency, and request routing overhead.
2. Benchmark results documented with a recommendation and rationale. Decision recorded as an ADR.
3. Chosen strategy implemented with: config loading on startup, incremental updates via LISTEN/NOTIFY (or SNS/SQS at Stage 3), and graceful handling of rebalancing when instances are added/removed.
4. If sharding is chosen: load balancer routing configured to direct requests to the correct instance. Health check failures cause automatic redistribution of the shard's configs to surviving instances within 60 seconds.
5. If full replication is chosen: verify that memory usage at 100K configs does not exceed 500MB per instance. Document the ceiling at which sharding becomes necessary.
6. Config loading validated as idempotent -- loading the same config twice produces identical state with no side effects.
7. Metrics exported: configs loaded per instance, config load time, cache hit/miss ratio, memory used by config cache.

#### Technical Notes

- The PRD infra document describes consistent hashing by server slug for Stage 3 EKS. However, full replication may be simpler and sufficient through Stage 2 (10K configs x ~1KB = ~10MB, trivially fits in 2GB instance memory). The benchmark should validate whether full replication works up to 100K before committing to sharding complexity.
- If full replication is chosen, config invalidation via LISTEN/NOTIFY fans out to all instances. This is a broadcast pattern -- every instance processes every invalidation event. At 1,000 config changes/minute, this produces 1,000 events x N instances. Measure CPU overhead.
- If consistent hashing is chosen, use a virtual node ring (e.g., 256 virtual nodes per instance) to minimize redistribution on scale events. The ALB does not natively support hash-based routing -- this requires either an Envoy sidecar for routing or an application-level routing layer.
- Whichever strategy is chosen, the credential injector sidecar does not cache configs -- it receives request skeletons from the gateway. No sharding impact on the sidecar.

#### Dependencies

- S-070 (statelessness contract confirms all config state is rebuildable)
- S-071 (connection pooling handles config reload queries at scale)

---

### S-073: Database Partitioning

**Priority:** P1
**Estimated Effort:** 5 story points
**Stage:** Stage 2-3 transition (when audit_log exceeds 10M rows)

#### Description

Implement table partitioning for high-growth tables to maintain query performance and enable efficient data lifecycle management. Without partitioning, `audit_log` and `request_logs` become multi-billion-row tables within months at Stage 3 volumes (50M requests/day = ~1.5B rows/month in request_logs). Full table scans for time-range queries, vacuum overhead, and index bloat will degrade performance.

Partition strategy:
- `audit_log`: range partition by `created_at` (monthly)
- `request_logs`: composite partition by `server_id` hash + `created_at` month (hash-range)
- `mcp_servers`: hash partition by `user_id` (for multi-tenant query isolation)

Implement automated partition management and archival of old partitions to cold storage.

#### Acceptance Criteria

1. `audit_log` table converted to range-partitioned by month on `created_at`. New partitions auto-created 30 days in advance. Existing data migrated to partitioned table with zero downtime (create new partitioned table, backfill, swap via rename).
2. `request_logs` table converted to composite partitioning: first-level hash by `server_id` (16 buckets), second-level range by `created_at` (monthly). Query `WHERE server_id = X AND created_at > Y` scans only the relevant partition.
3. `mcp_servers` table converted to hash-partitioned by `user_id` (32 buckets). Query `WHERE user_id = X` reads a single partition. Cross-user queries (admin dashboard) scan all partitions but benefit from partition-level parallelism.
4. Automated partition management: a cron job (or pg_partman extension) creates new monthly partitions and detaches partitions older than the configured retention period (default: 90 days for request_logs, 365 days for audit_log).
5. Detached partitions exported to S3 as Parquet files (via `pg_dump` or `COPY TO` with post-processing). S3 lifecycle policy moves exports to Glacier after 30 days.
6. Query performance validated: time-range queries on `audit_log` (last 7 days for a specific server) execute in <10ms at p95 with 100M+ total rows. Verified via `EXPLAIN ANALYZE`.
7. Partition pruning confirmed active: `EXPLAIN` output shows only relevant partitions scanned for filtered queries.
8. Runbook documented for: manual partition creation, emergency detach of a problematic partition, restoration of an archived partition from S3.

#### Technical Notes

- PostgreSQL native declarative partitioning (available since PG 10) is preferred over pg_partman for the core partitioning. pg_partman can manage the automated creation/detach lifecycle.
- The zero-downtime migration approach: create a new partitioned table (`audit_log_v2`), set up a trigger on the old table to dual-write to both, backfill historical data in batches, validate row counts, then rename tables in a transaction (`audit_log` -> `audit_log_old`, `audit_log_v2` -> `audit_log`). The expand-contract pattern from S-085 applies here.
- Hash partitioning `mcp_servers` by `user_id` is a trade-off: it speeds up per-user queries but complicates queries that filter by `slug` (the MCP routing path). Ensure the `slug` unique index spans all partitions (global index). PostgreSQL supports this natively for unique indexes that include the partition key.
- At Aurora (Stage 3), partition management is the same -- Aurora is PostgreSQL-compatible. Aurora's parallel query can further accelerate cross-partition scans.

#### Dependencies

- S-071 (connection pooling must handle migration query load)
- S-085 (zero-downtime deployment for schema migration)

---

### S-074: Redis Cache Layer

**Priority:** P0
**Estimated Effort:** 8 story points
**Stage:** Stage 2 (deploy with ECS Fargate migration)

#### Description

Introduce Redis (ElastiCache) as a shared cache and coordination layer across gateway instances. At Stage 1, all caching is in-process (single instance). At Stage 2 with multiple instances, a shared cache is needed for: cross-instance config caching, distributed rate limiting counters, session data, and pub/sub for config invalidation at scale.

Redis supplements (not replaces) PostgreSQL LISTEN/NOTIFY for config invalidation at Stage 2. At Stage 3 (>5,000 servers), SNS/SQS replaces both for fan-out (see S-088).

The system must degrade gracefully when Redis is unavailable -- fall back to PostgreSQL for config reads and in-process rate limiting.

#### Acceptance Criteria

1. Redis deployed as ElastiCache `cache.t4g.micro` (Stage 2) or Redis Cluster with 3 shards (Stage 3). Connection configured via environment variable.
2. Config cache: server configs cached in Redis with key `config:{server_slug}`, TTL 300 seconds. Cache-aside pattern: gateway checks Redis first, falls back to PostgreSQL on miss, writes result to Redis. Cache invalidation on config change via Redis pub/sub channel `config:invalidate`.
3. Rate limiting: per-server and per-user counters stored in Redis using `INCR` with TTL (sliding window). Atomic operations ensure accuracy under concurrent access. Implementation matches the rate limit tiers defined in the PRD (free: 100/min, pro: 1000/min, enterprise: 10000/min).
4. Session cache: platform session tokens cached in Redis (TTL matches Clerk session TTL of 7 days). Reduces Clerk API calls on every authenticated request.
5. Fallback behavior verified: when Redis is unreachable, the gateway continues operating with degraded performance. Config reads fall back to PostgreSQL (higher latency but functional). Rate limiting falls back to in-process counters (per-instance, not distributed -- documented as degraded). Session validation falls back to Clerk API.
6. Redis connection pool configured: min 5, max 50 connections per gateway instance. Connection health checks every 10 seconds. Automatic reconnection on failure.
7. Metrics exported: cache hit/miss ratio (per key prefix), Redis latency (p50/p95/p99), connection pool utilization, pub/sub message throughput, fallback activation count.
8. Load test: Redis failure injected during 5,000 req/s sustained load. Gateway continues serving requests with <500ms increase in p95 latency. Zero dropped requests due to Redis failure.

#### Technical Notes

- **Redis client:** Use `fred` (Rust Redis client with cluster support, connection pooling, and async/await). Alternatively `redis-rs` with `deadpool-redis` for pooling.
- **Cache-aside vs read-through:** Cache-aside is simpler and gives the application explicit control over what gets cached. Read-through requires a cache-population callback, which complicates fallback logic.
- **Rate limiting in Redis:** Use a Lua script for atomic sliding window rate limiting: `MULTI / ZADD / ZREMRANGEBYSCORE / ZCARD / EXPIRE / EXEC`. This avoids race conditions that `INCR` alone cannot handle for sliding windows. Token bucket is an alternative but sliding window is simpler to reason about.
- **Pub/sub for config invalidation:** At Stage 2 (4-8 instances), Redis pub/sub is sufficient. At Stage 3 (20-100 pods), pub/sub fan-out to 100 subscribers is still efficient but SNS/SQS (S-088) provides durability guarantees that pub/sub lacks (messages lost if subscriber is down during publish).
- **Serialization:** Use MessagePack for Redis values (faster than JSON, smaller than Protobuf for small payloads). Config objects are typically 500 bytes-2KB.
- The infra doc specifies ElastiCache `cache.t4g.micro` at $12/month for Stage 2. At Stage 3, upgrade to Redis Cluster with 3 shards for HA and throughput.

#### Dependencies

- S-070 (statelessness contract -- Redis is shared state, must be treated as rebuildable cache, not source of truth)
- S-071 (PostgreSQL fallback queries must be optimized for cache-miss scenarios)

---

### S-075: Horizontal Auto-Scaling

**Priority:** P0
**Estimated Effort:** 8 story points
**Stage:** Stage 2 (ECS) and Stage 3 (EKS)

#### Description

Implement auto-scaling for gateway instances based on connection load, CPU, memory, and request queue depth. The gateway must scale out rapidly when traffic increases (new instance serving requests within 60 seconds) and scale in gracefully (drain existing connections before terminating an instance). The minimum instance count is 2 (for redundancy). The target is 80% connection utilization per instance before triggering scale-out.

At Stage 2, this uses ECS Service Auto Scaling. At Stage 3, this uses Kubernetes HPA with KEDA for custom metrics.

#### Acceptance Criteria

1. Auto-scaling policy configured with the following triggers (any one triggers scale-out):
   - Active connections per instance > 80% of configured maximum (default max: 500 connections/instance)
   - CPU utilization > 70% sustained for 2 minutes
   - Memory utilization > 80% sustained for 2 minutes
   - Request queue depth > 100 (requests waiting for a connection slot)
2. Scale-out: new instance passes health check and begins receiving traffic within 60 seconds of scaling decision. Config cache is populated (cold start) within this window.
3. Scale-in: connections are drained for 30 seconds before instance termination. Active SSE/Streamable HTTP sessions receive a graceful close notification. No requests are dropped during scale-in.
4. Minimum instance count: 2 (even during low-traffic periods). Maximum instance count: configurable, default 20 (Stage 2) / 100 (Stage 3).
5. Scaling cooldown: 60 seconds after scale-out, 300 seconds after scale-in. Prevents oscillation.
6. Health check endpoint `/healthz` returns: (a) 200 when the instance is ready to serve traffic (config cache loaded, database connection pool initialized, Redis connected), (b) 503 during startup or draining.
7. Scaling events logged and alerted: scale-out events generate an INFO alert, repeated scale-out within 5 minutes generates a WARNING alert (potential capacity issue).
8. Load test validated: ramp from 0 to 50,000 concurrent connections over 10 minutes. Auto-scaler adds instances to maintain <80% utilization per instance. p95 latency stays below 200ms throughout the ramp. Zero dropped connections during scaling events.

#### Technical Notes

- **ECS (Stage 2):** Use ECS Service Auto Scaling with target tracking policy on a custom CloudWatch metric (active connections per task). Publish the metric from each gateway instance every 10 seconds. ECS launches new Fargate tasks and registers them with the ALB target group.
- **EKS (Stage 3):** Use HPA with custom metrics from Prometheus (via prometheus-adapter) or KEDA with a Prometheus scaler. KEDA supports scaling to zero (not applicable here due to min=2) and scaling based on arbitrary metrics (active connections, SQS queue depth).
- **Graceful shutdown:** On SIGTERM, the gateway: (1) deregisters from load balancer (stop accepting new connections), (2) sends SSE close events to active clients, (3) waits up to 30 seconds for in-flight requests to complete, (4) flushes audit log buffer (S-071), (5) exits. ECS `stopTimeout` and Kubernetes `terminationGracePeriodSeconds` must be set to at least 45 seconds.
- **Cold start optimization:** On startup, load all configs assigned to this instance in a single bulk query (`SELECT * FROM mcp_servers WHERE status = 'active'`). At 10K configs, this is ~10MB and completes in <5 seconds. Prefill Redis cache entries for loaded configs.
- **Spot instances (Stage 3):** KEDA can be configured to prefer scaling on-demand instances first and add spot instances during sustained high load. Spot interruptions trigger the same graceful shutdown path.

#### Dependencies

- S-070 (stateless gateway -- instances must be interchangeable)
- S-072 (config sharding -- determines what configs each instance loads on startup)
- S-076 (load balancer -- routes traffic to scaled instances)

---

### S-076: Load Balancer Optimization

**Priority:** P1
**Estimated Effort:** 5 story points
**Stage:** Stage 2 (ECS Fargate with ALB)

#### Description

Configure and optimize the AWS Application Load Balancer for the gateway workload. The gateway serves a mix of short-lived HTTP requests (`tools/list`, `tools/call`) and long-lived connections (SSE streams). The ALB must handle both patterns efficiently, provide health checking, support connection draining during deployments, and enable cross-zone load balancing for AZ resilience.

#### Acceptance Criteria

1. ALB configured with path-based routing:
   - `/{server-slug}/mcp/*` routes to MCP Router target group
   - `/api/*` routes to Platform API target group
   - `/*` (default) returns 404 with a JSON error body
2. Health check configuration: path `/healthz`, interval 5 seconds, healthy threshold 2, unhealthy threshold 2, timeout 3 seconds. Unhealthy instances removed from rotation within 10 seconds.
3. Connection draining enabled with 30-second timeout. During deployments, in-flight requests complete before old instances are terminated. Verified with a long-running SSE connection during a rolling deploy.
4. TLS termination at ALB using ACM certificate. TLS 1.2 minimum (TLS 1.3 preferred). HTTPS listener on port 443 only; HTTP listener on port 80 redirects to HTTPS.
5. WebSocket and SSE support verified: ALB forwards `Connection: Upgrade` headers for WebSocket. SSE streams (chunked transfer encoding with `text/event-stream`) pass through without buffering. Idle timeout set to 3600 seconds (1 hour) for long-lived MCP sessions.
6. Cross-zone load balancing enabled. Traffic distributed evenly across all AZs regardless of instance count per AZ.
7. ALB access logs enabled, stored in S3 with 30-day retention. Log format includes: client IP, request path, response code, target response time, request processing time.
8. Request routing metrics exported to CloudWatch: requests per target, target response time (p50/p95/p99), HTTP 4xx/5xx counts, active connection count, new connection rate.
9. Sticky sessions explicitly disabled (verified in target group configuration). Any instance can serve any request.

#### Technical Notes

- **ALB idle timeout:** Default is 60 seconds, which will kill SSE connections after 60 seconds of no data. Set to 3600 seconds (maximum). Gateway should send SSE keepalive comments (`: keepalive\n\n`) every 30 seconds to prevent intermediate proxies (Cloudflare) from closing the connection.
- **ALB connection limits:** ALB supports ~100,000 concurrent connections per node. AWS auto-scales ALB nodes based on traffic. For peak concurrent connections of 100,000, pre-warm the ALB by contacting AWS support or by gradually ramping traffic.
- **Slow start:** Enable ALB slow start (30 seconds) on the MCP Router target group. New instances receive a linearly increasing share of traffic over 30 seconds, giving the config cache time to warm.
- **Security groups:** ALB security group allows inbound 443 from 0.0.0.0/0 (or Cloudflare IP ranges only if using Cloudflare proxy). MCP Router security group allows inbound only from the ALB security group. RDS security group allows inbound only from the MCP Router and Platform API security groups.
- **WAF:** Attach AWS WAF to the ALB for SQL injection, XSS, and rate limiting at the edge. This supplements application-level rate limiting (S-087).

#### Dependencies

- S-075 (auto-scaling registers new instances with the ALB target group)

---

### S-077: Multi-Region Deployment

**Priority:** P2
**Estimated Effort:** 13 story points
**Stage:** Stage 3 (EKS, 10K+ servers)

#### Description

Deploy the gateway platform across three regions: US-East (us-east-1, primary), EU-West (eu-west-1), and APAC (ap-southeast-1, Singapore). Each region runs an independent gateway cluster with local read replicas for low-latency reads. All writes go to the primary region (US-East) and replicate asynchronously to secondary regions. Users are routed to the nearest region via geo-DNS.

This is the largest and most complex story in the epic. It transforms the platform from a single-region service to a globally distributed system with cross-region data replication, region-aware routing, and coordinated deployments.

#### Acceptance Criteria

1. Gateway clusters deployed in all three regions, each running: EKS cluster (min 2 nodes), MCP Router pods (min 3), Platform API pods (min 2), Redis replica, Aurora read replica.
2. Geo-DNS routing configured (Route53 latency-based routing or Cloudflare load balancing). Users in EU are routed to EU-West with <50ms DNS-to-first-byte overhead vs routing to US-East.
3. Read path: `tools/list` and `tools/call` served from the local region using the local Aurora read replica and local Redis cache. No cross-region calls on the read path.
4. Write path: config creation, config updates, credential storage, and user management writes are forwarded to the primary region (US-East). Write latency from EU: <300ms (includes cross-region round-trip). Write latency from APAC: <500ms.
5. Replication lag between primary and secondary Aurora replicas measured and alerted. Target: <1 second under normal load. Alert at >5 seconds.
6. Regional failover tested: simulate US-East outage. EU-West Aurora replica promoted to writer within 60 seconds. DNS failover routes US traffic to EU-West. Platform operational (degraded latency) within 5 minutes. Document the failover runbook.
7. Config invalidation propagates across regions: a config change in US-East is reflected in EU-West and APAC gateway caches within 5 seconds (replication lag + cache invalidation).
8. Deployment pipeline deploys to all regions sequentially: US-East first (canary), then EU-West, then APAC. A failed deployment in US-East halts rollout to other regions.
9. Per-region health dashboard showing: request volume, error rate, latency, Aurora replication lag, Redis sync status.

#### Technical Notes

- **Aurora Global Database** handles cross-region replication at the database layer (see S-078). This story focuses on the application and infrastructure layers.
- **Write forwarding:** Two approaches: (a) Aurora Global Database write forwarding (available in Aurora PostgreSQL 13.4+) transparently forwards writes from read replicas to the primary. Adds ~100ms per write. (b) Application-level write routing: the gateway detects write operations and routes them to the primary region's API endpoint. Option (a) is simpler but has caveats (no support for DDL, advisory locks). Use option (a) for simple writes, option (b) for complex transactions.
- **Redis cross-region:** ElastiCache Global Datastore provides cross-region Redis replication with <1 second lag. Alternatively, each region runs an independent Redis instance and cache warming happens via the config invalidation pipeline (S-088).
- **Cost:** Multi-region roughly triples infrastructure costs. Stage 3 baseline ~$6,700/month becomes ~$15,000-20,000/month with three regions. Start with two regions (US + EU) and add APAC when user demand justifies it.
- **Terraform:** Use Terraform workspaces or Terragrunt to manage per-region infrastructure from a single codebase. Region-specific variables (instance sizes, node counts) parameterized.

#### Dependencies

- S-078 (Aurora Global Database -- provides the cross-region data layer)
- S-075 (auto-scaling -- each region scales independently)
- S-076 (load balancer -- each region has its own ALB/NLB)
- S-088 (config propagation -- must work cross-region)
- S-085 (zero-downtime deployments -- sequential multi-region rollout)

---

### S-078: Aurora Global Database

**Priority:** P1
**Estimated Effort:** 8 story points
**Stage:** Stage 3 (EKS migration)

#### Description

Migrate from single-region RDS PostgreSQL (Stage 2) to Aurora PostgreSQL Global Database. The primary cluster in US-East handles all writes. Read replicas in EU-West and APAC serve read traffic for their respective regions. Replication lag must be under 1 second. Automatic failover must promote a secondary region to writer within 60 seconds of a primary region failure.

This migration must be performed with zero downtime and zero data loss.

#### Acceptance Criteria

1. Aurora Global Database cluster created with: primary cluster in us-east-1 (`db.r6g.xlarge`, 4 vCPU / 32 GB), secondary clusters in eu-west-1 and ap-southeast-1 (`db.r6g.large`, 2 vCPU / 16 GB each).
2. Data migrated from RDS PostgreSQL to Aurora using AWS DMS (Database Migration Service) with ongoing replication. Cutover performed during a maintenance window with <5 minutes of read-only mode. Zero data loss verified by row count comparison.
3. Connection strings managed per region via environment variables or AWS Secrets Manager. Gateway in each region connects to the local Aurora endpoint for reads and the global writer endpoint for writes.
4. Replication lag continuously monitored via CloudWatch metric `AuroraGlobalDBReplicationLag`. Alert at >2 seconds, critical alert at >5 seconds. Normal operating range: <1 second.
5. Failover tested: detach primary region from the global cluster. Secondary region in EU-West promoted to writer. Application reconnects within 60 seconds. Verify writes succeed in the new primary. Verify APAC read replica begins replicating from the new primary.
6. Connection pooling (RDS Proxy) configured per region: one proxy per cluster endpoint. Gateway connects to RDS Proxy, not directly to Aurora. Proxy handles connection multiplexing for 100+ pods.
7. Performance validated: read query latency from EU-West (via local replica) <5ms at p95 for config lookups. Write query latency from EU-West (forwarded to US-East primary) <150ms at p95.
8. Backup strategy: Aurora automated backups (35-day retention) on primary. Cross-region snapshot copy to a fourth region (us-west-2) for catastrophic multi-region failure.

#### Technical Notes

- **Aurora vs standard RDS:** Aurora's storage layer replicates data across 3 AZs automatically (6 copies of data). Global Database adds cross-region replication on top. This is fundamentally different from standard RDS read replicas (which use PostgreSQL streaming replication and have higher lag).
- **DMS migration:** Create a DMS replication instance in the same VPC as the source RDS. Configure a full-load + CDC (change data capture) task. Monitor the DMS task for errors. After full load completes and CDC catches up (lag = 0), perform the cutover: stop application writes, wait for DMS lag to reach 0, switch connection strings, resume writes against Aurora.
- **Write forwarding vs application routing:** Aurora Global Database supports write forwarding from read replicas. However, it has limitations: no support for `COPY`, advisory locks, or temp tables. For the gateway's write patterns (simple INSERTs and UPDATEs), write forwarding is sufficient. Complex batch operations (audit log archival) must be routed to the writer explicitly.
- **Cost:** Aurora `db.r6g.xlarge` primary ~$600/month, two `db.r6g.large` secondaries ~$300/month each. Aurora storage: $0.10/GB/month. RDS Proxy: $18/month per proxy. Total database layer: ~$1,250/month vs ~$130/month for Stage 2 RDS.

#### Dependencies

- S-071 (connection pooling abstraction -- read/write split must work with Aurora endpoints)
- S-073 (partitioning -- must be compatible with Aurora PostgreSQL)

---

### S-079: CDN and Edge Caching

**Priority:** P1
**Estimated Effort:** 5 story points
**Stage:** Stage 2-3 (deploy incrementally)

#### Description

Implement CDN and edge caching to reduce gateway load and improve latency for cacheable responses. The React SPA is served via Cloudflare CDN (static assets). MCP `tools/list` responses are cached at the edge with a cache key of `server_id` and a TTL of 60 seconds, invalidated on config change. Frequently accessed server configs are cached at the API layer with appropriate cache headers.

The target is >90% cache hit rate for `tools/list` responses, which are the most frequent MCP request type and return the same data for all clients of a given server.

#### Acceptance Criteria

1. React SPA served via Cloudflare CDN with: immutable asset caching (content-hashed filenames, `Cache-Control: public, max-age=31536000, immutable`), `index.html` cached with `Cache-Control: no-cache` (revalidate on every request), Brotli compression enabled.
2. `tools/list` responses include `Cache-Control: public, max-age=60` and `ETag` header (hash of config version). Cloudflare caches the response at edge. Subsequent requests within 60 seconds served from edge without hitting the gateway.
3. Cache invalidation on config change: when a user updates server config, the gateway calls the Cloudflare API to purge the cache for that server's `tools/list` URL. Purge completes within 5 seconds.
4. `tools/call` responses are NOT cached (they depend on upstream API state and user-provided parameters). `Cache-Control: no-store` header set explicitly.
5. Cache hit rate monitored via Cloudflare analytics. Target: >90% for `tools/list` after 30 days of operation. Alert if cache hit rate drops below 70%.
6. API responses for server config retrieval (Platform API, not MCP path) include `ETag` and support `If-None-Match` conditional requests. Gateway returns 304 Not Modified when config has not changed.
7. Total CDN cost remains under $20/month at Stage 2 volumes (Cloudflare Pro plan).
8. Edge caching does not interfere with per-server Bearer token authentication. Cached responses must not be served to unauthenticated clients. Implement via `Vary: Authorization` header or Cloudflare cache rules that bypass cache for requests without valid auth.

#### Technical Notes

- **Cache key for tools/list:** `{scheme}://{host}/{server-slug}/mcp/tools/list`. The response is identical for all authenticated clients of a given server, so caching is safe as long as auth is validated before the cache lookup. Use Cloudflare Cache Rules to: (a) only cache if the origin returned 200, (b) vary by Authorization header, (c) respect the origin's Cache-Control header.
- **Cache invalidation:** Cloudflare purge-by-URL API: `POST /zones/{zone_id}/purge_cache` with `{"files": ["https://mcp.example.com/{slug}/mcp/tools/list"]}`. Integrate into the config update handler (same code path that sends LISTEN/NOTIFY).
- **Security consideration:** If `tools/list` is cached at the edge with `Vary: Authorization`, each unique Bearer token generates a separate cache entry. This is acceptable (bounded by number of active tokens per server, typically 1-5). Without `Vary`, a cached response for one token could be served to a different token -- this is a security issue only if different tokens should see different tool lists (not the case in current design, all tokens for a server see the same tools).
- **SPA caching:** Use Cloudflare Page Rules or Cache Rules. Static assets under `/assets/` get long-lived caching. The root `index.html` uses `stale-while-revalidate` for near-instant loads with background refresh.

#### Dependencies

- S-076 (load balancer -- TLS and routing must be compatible with Cloudflare proxy)

---

### S-080: Credential Management at Scale

**Priority:** P1
**Estimated Effort:** 8 story points
**Stage:** Stage 2-3 transition

#### Description

Migrate credential encryption from application-level AES-256-GCM (Stage 1, single encryption key in environment variable) to AWS KMS envelope encryption with per-user Data Encryption Keys (DEKs). This provides: key rotation without re-encrypting all credentials, per-user isolation (a compromised DEK exposes only one user's credentials), and an auditable key management trail via CloudTrail.

The target is <5ms overhead for credential decryption on the hot path (including DEK cache lookup).

#### Acceptance Criteria

1. AWS KMS Customer Managed Key (CMK) created for the platform. Key policy restricts usage to the gateway IAM role and the credential injector sidecar IAM role. No other principals can decrypt.
2. Per-user DEK generated via KMS `GenerateDataKey`. DEK stored encrypted (envelope encryption) in the `user_encryption_keys` table. Plaintext DEK never persisted to disk or database.
3. Credential encryption flow: (a) retrieve user's encrypted DEK from database, (b) decrypt DEK via KMS, (c) use plaintext DEK to encrypt the credential with AES-256-GCM, (d) store encrypted credential + IV in the `credentials` table.
4. DEK caching: plaintext DEKs cached in-memory with 5-minute TTL. Cache is per-instance (not shared via Redis -- plaintext keys must not traverse the network). Cache eviction on TTL expiry or instance restart.
5. Key rotation: rotating the CMK in KMS automatically applies to new DEK generations. Existing DEKs are re-encrypted under the new CMK version via a background job (S-081). No credential re-encryption needed (DEKs remain the same, only their envelope changes).
6. Migration from Stage 1 encryption: background job decrypts all credentials using the old application-level key, re-encrypts using the user's DEK, and updates the database. Migration is idempotent (safe to re-run). Old encryption format detected by a version byte prefix on the ciphertext.
7. Credential decryption on the hot path measured: <5ms at p95 (DEK cache hit) and <50ms at p95 (DEK cache miss, requires KMS call). Cache hit rate >99% in steady state.
8. CloudTrail logging enabled for all KMS API calls. Anomaly detection: alert if KMS decrypt calls exceed 2x the expected rate (potential credential exfiltration attempt).
9. Credential injector sidecar updated to use the new encryption path. Gateway still never holds plaintext credentials -- the sidecar performs decryption.

#### Technical Notes

- **KMS pricing:** $1/month per CMK + $0.03 per 10,000 API calls. At 50M requests/day with 5-minute DEK cache and ~10,000 users, expect ~2,880 KMS calls/day (10,000 users / 5-minute TTL = 2,000 refreshes/hour = 2,880/day assuming not all users are active). Cost: negligible.
- **DEK storage:** The `user_encryption_keys` table stores: `user_id` (FK), `encrypted_dek` (bytea, KMS-encrypted), `key_version` (integer, tracks CMK rotation), `created_at`. One row per user.
- **Envelope encryption pattern:** The classic envelope pattern -- KMS never sees the actual credentials. KMS only encrypts/decrypts the DEK. This limits KMS API call volume and keeps credential plaintext within the sidecar process boundary.
- **HSM backing:** KMS keys are backed by FIPS 140-2 Level 3 HSMs. This satisfies SOC 2 and most enterprise compliance requirements without running dedicated HSMs.
- **Vault alternative:** The infra doc specifies HashiCorp Vault for Stage 3. KMS envelope encryption is simpler, cheaper, and sufficient unless customers require Vault-specific features (dynamic database credentials, PKI). Recommend KMS for Stage 2-3 and Vault only if enterprise customers demand it.

#### Dependencies

- S-081 (async job queue -- used for DEK re-encryption during key rotation and migration)
- S-071 (connection pooling -- migration job queries all credentials)

---

### S-081: Async Job Queue

**Priority:** P1
**Estimated Effort:** 5 story points
**Stage:** Stage 2

#### Description

Implement a background job processing system for operations that should not block the request path: credential rotation, audit log archival, usage metering aggregation, email notifications, and Docker export builds. The queue must provide at-least-once delivery, dead letter handling for failed jobs, and visibility into job status.

#### Acceptance Criteria

1. Job queue infrastructure deployed: SQS (primary) with dead letter queue (DLQ), or Redis-based queue (if SQS latency is unacceptable for time-sensitive jobs). Decision documented as ADR.
2. Job types implemented with dedicated handlers:
   - `credential_rotation`: decrypt and re-encrypt credentials during key rotation (S-080)
   - `audit_log_archival`: export old audit log partitions to S3 (S-073)
   - `usage_metering`: aggregate request counts into billing-period buckets (S-082)
   - `email_notification`: send transactional emails (server created, credential expiring, usage threshold)
   - `docker_export`: build Docker image from server config (Export feature)
3. At-least-once delivery guarantee: jobs are not removed from the queue until the handler acknowledges completion. Handlers must be idempotent (re-processing the same job produces the same result without side effects).
4. Dead letter queue: jobs that fail 3 times are moved to the DLQ. DLQ depth monitored. Alert when DLQ depth > 0. Manual retry mechanism (re-drive from DLQ to main queue).
5. Job visibility: each job has a unique ID, creation timestamp, status (pending, processing, completed, failed), attempt count, and error message (on failure). Job status queryable via admin API.
6. Job processing latency: jobs picked up within 5 seconds of enqueue. Processing SLA per job type documented (credential_rotation: <60s, email: <30s, docker_export: <5min).
7. Worker auto-scaling: at Stage 2, a single worker process handles all job types. At Stage 3, workers scale based on queue depth (SQS-based KEDA scaler or ECS step scaling).
8. Graceful shutdown: worker finishes in-progress job before exiting. Maximum job execution timeout: 10 minutes. Jobs exceeding timeout are returned to the queue.

#### Technical Notes

- **SQS vs Redis queue:** SQS provides: durability (messages persisted across AZ failures), visibility timeout (message hidden during processing), dead letter queue (native), and cost ($0.40 per million requests -- negligible). Redis-based queues (BullMQ pattern with Rust implementation) provide: lower latency (<1ms vs ~20ms for SQS), priority queues, and cron-like scheduling. Recommendation: SQS for durability-critical jobs (credential rotation, audit archival), Redis for latency-sensitive jobs (config invalidation, real-time metering).
- **Worker implementation:** A separate Rust binary (or a mode flag on the existing gateway binary: `--mode worker`) that: (1) polls SQS/Redis for jobs, (2) deserializes the job payload, (3) dispatches to the appropriate handler, (4) acknowledges or retries. Use the same database connection pool as the gateway.
- **Idempotency:** Each job carries an idempotency key (e.g., `credential_rotation:{user_id}:{rotation_id}`). Handlers check a `processed_jobs` table before executing. If the key exists, skip execution and acknowledge the job.
- **Docker export builds:** These are CPU-intensive and long-running. Isolate them on dedicated worker instances (or Fargate Spot tasks) to avoid impacting the gateway.

#### Dependencies

- S-071 (database connection pooling -- workers share the pool)
- S-074 (Redis -- used as queue backend for latency-sensitive jobs)

---

### S-082: Usage Metering and Billing Pipeline

**Priority:** P2
**Estimated Effort:** 8 story points
**Stage:** Stage 2-3

#### Description

Build a metering pipeline that tracks per-server and per-user API call counts, aggregates them into billing-period windows, and feeds the results to Stripe for usage-based billing. The pipeline must handle: clock skew between gateway instances, duplicate events (at-least-once delivery from the job queue), late-arriving data (events that arrive after the aggregation window closes), and edge cases like mid-period plan changes.

#### Acceptance Criteria

1. Every MCP `tools/call` request generates a metering event: `{server_id, user_id, timestamp, request_type, response_status, latency_ms}`. Events are buffered in-memory and flushed to the metering store every 5 minutes (or 10,000 events, whichever comes first).
2. Metering store: time-series table `usage_events(user_id, server_id, window_start, call_count, error_count, total_latency_ms)` with 5-minute aggregation windows. Keyed on `(user_id, server_id, window_start)` for upsert.
3. Aggregation is idempotent: re-processing the same raw events produces the same aggregated counts (use `INSERT ... ON CONFLICT DO UPDATE SET call_count = EXCLUDED.call_count` with event-sourced counts, not increments).
4. Stripe integration: at the end of each billing period (monthly), a billing job (S-081) reads aggregated usage for each user, submits usage records to Stripe via the Metered Billing API (`stripe.subscription_items.create_usage_record`). Idempotency key prevents double-billing on retry.
5. Late-arriving data handling: events arriving after the aggregation window closes update the next window's counts. A reconciliation job runs daily and adjusts any discrepancies between raw events and aggregated counts.
6. Clock skew tolerance: events are bucketed by server-side timestamp (gateway instance clock). NTP must be configured on all instances with <1 second drift. Events with timestamps >5 minutes in the future are rejected. Events >1 hour old are flagged for review.
7. User-facing usage dashboard: per-server and aggregate call counts for the current billing period, updated every 15 minutes. Shows: total calls, calls remaining (vs plan limit), graph of daily usage.
8. Alert when a user reaches 80% and 100% of their plan limit. At 100%, behavior depends on plan: free tier returns 429, paid tiers allow overage (billed at per-call rate).

#### Technical Notes

- **Why not a dedicated time-series database (TimescaleDB, InfluxDB)?** At Stage 2 volumes (2M requests/day = ~400K aggregated rows/day in 5-min windows), PostgreSQL with partitioned tables handles this comfortably. TimescaleDB is a PostgreSQL extension and could be added later if query patterns demand it. At Stage 3 (50M requests/day), consider Kinesis Data Firehose -> S3 -> Athena for analytics, keeping PostgreSQL for billing-critical aggregation only.
- **Stripe metered billing:** Use `stripe.subscription_items.create_usage_record` with `action: 'set'` (not `increment`) and an idempotency key of `{user_id}:{billing_period}:{submission_attempt}`. This makes resubmission safe.
- **Plan change mid-period:** Stripe prorates automatically. The metering pipeline does not need to handle proration -- just report total usage for the period.
- **Buffer flush on shutdown:** Same pattern as audit log batching (S-071). Bounded channel with background flush task. Drain on SIGTERM.

#### Dependencies

- S-081 (async job queue -- billing job runs as a scheduled job)
- S-071 (database optimization -- metering queries must not compete with hot path)
- S-073 (partitioning -- usage_events table should be partitioned by month)

---

### S-083: Observability at Scale

**Priority:** P1
**Estimated Effort:** 8 story points
**Stage:** Stage 2 (basic) and Stage 3 (full)

#### Description

Implement a comprehensive observability stack spanning distributed tracing, metrics aggregation, and centralized logging. At Stage 1, observability is minimal (Sentry + BetterStack). At Stage 2+, the platform needs: request-level tracing across gateway -> sidecar -> upstream API, per-server metrics dashboards, structured log aggregation, and alerting with on-call integration.

#### Acceptance Criteria

1. **Distributed tracing:** OpenTelemetry SDK integrated into the Rust gateway and credential injector sidecar. Every MCP request generates a trace with spans for: request parsing, config lookup (cache hit/miss), credential injection (sidecar call), upstream API call, response transformation, response serialization. Trace ID propagated to upstream APIs via `traceparent` header.
2. **Trace backend:** Jaeger (self-hosted on EKS, Stage 3) or Datadog APM (managed, Stage 2-3). Traces retained for 7 days. Searchable by: trace ID, server_id, user_id, HTTP status code, latency threshold.
3. **Metrics pipeline:** Prometheus metrics exported from the gateway via `/metrics` endpoint. Key metrics: `gateway_request_duration_seconds` (histogram, labels: server_id, method, status), `gateway_active_connections` (gauge), `gateway_config_cache_hits_total` / `gateway_config_cache_misses_total` (counters), `gateway_upstream_duration_seconds` (histogram), `gateway_credential_decrypt_duration_seconds` (histogram), `gateway_connection_pool_active` (gauge, per pool).
4. **Dashboards:** Grafana dashboards for: (a) platform overview (total requests/sec, error rate, p50/p95/p99 latency, active connections, instance count), (b) per-server detail (request rate, error rate, upstream latency, cache hit rate), (c) database health (connection pool, query latency, replication lag), (d) Redis health (memory, hit rate, connection count).
5. **Log aggregation:** Structured JSON logs shipped to CloudWatch Logs (Stage 2) or Loki (Stage 3). Log fields: `timestamp`, `level`, `trace_id`, `server_id`, `user_id`, `method`, `path`, `status_code`, `latency_ms`, `error`. Log retention: 30 days hot, 90 days cold (S3).
6. **Alerting:** PagerDuty integration for critical alerts. Alert definitions:
   - Critical: gateway error rate >5% for 5 minutes, database connection pool exhausted, zero healthy instances in any region
   - Warning: p95 latency >500ms for 10 minutes, cache hit rate <50%, replication lag >5 seconds, disk >80%
   - Info: scale-out event, deployment started, scheduled maintenance
7. **Trace-to-logs correlation:** clicking a trace in Jaeger/Datadog links to the corresponding structured log entries via shared `trace_id`.
8. **Cost:** Observability stack cost remains under $100/month at Stage 2 (Grafana Cloud Free + CloudWatch + Sentry Team). Budget up to $500/month at Stage 3 for Datadog or self-hosted stack.

#### Technical Notes

- **OpenTelemetry in Rust:** Use `tracing` crate (standard in the Rust ecosystem) with `tracing-opentelemetry` bridge. This integrates with the existing `tracing` instrumentation and exports spans in OTLP format.
- **Prometheus in Rust:** Use `prometheus` crate or `metrics` crate with `metrics-exporter-prometheus`. Expose on a separate port (9090) or a subpath (`/metrics`) that is not exposed via the public ALB.
- **Cardinality warning:** `server_id` as a metric label creates high cardinality (100K+ label values at Stage 3). Prometheus handles this poorly. Options: (a) use `server_id` only in traces and logs, not metrics, (b) aggregate metrics by user_id or plan tier instead, (c) use Datadog which handles high cardinality natively. Recommendation: at Stage 2 (<10K servers), `server_id` labels are fine. At Stage 3, switch to trace-based per-server analysis and aggregate metrics by tier.
- **Sentry:** Keep Sentry for error tracking (stack traces, error grouping). Do not attempt to replace Sentry with generic log aggregation -- Sentry's error deduplication is uniquely valuable.
- **Log redaction:** Implement a log redaction filter that strips: Bearer tokens, API keys, credential payloads, and any field matching patterns like `authorization`, `api_key`, `secret`, `password`. This is a security requirement, not optional.

#### Dependencies

- S-076 (load balancer -- ALB access logs feed into the log aggregation pipeline)
- S-074 (Redis -- Redis metrics included in dashboards)

---

### S-084: Chaos Engineering and Load Testing

**Priority:** P2
**Estimated Effort:** 8 story points
**Stage:** Stage 3 (run against staging environment)

#### Description

Build a load testing and chaos engineering framework to validate system resilience under extreme conditions. The load test must simulate 100,000 concurrent MCP sessions with realistic traffic patterns. Chaos tests must verify the system recovers from: instance failures, network partitions, database failover, and Redis failures. Tests run automatically in staging on a weekly schedule.

#### Acceptance Criteria

1. **Load test framework:** Script (using k6, Locust, or Gatling) that simulates MCP client behavior: connect via Streamable HTTP, call `tools/list`, call `tools/call` with realistic parameters, maintain SSE connection for 5-60 minutes. Configurable: concurrent users, ramp rate, duration, target server distribution (zipf -- 20% of servers receive 80% of traffic).
2. **Load test baseline:** Run at 100K concurrent connections against staging (equivalent to production Stage 3 sizing). Record: p50/p95/p99 latency, error rate, throughput (requests/sec), instance count (auto-scaled), CPU/memory utilization per instance. Results stored as a versioned artifact.
3. **Chaos tests implemented:**
   - Kill random gateway pod (1 of N): verify zero dropped requests (connections redistributed within 30 seconds)
   - Kill 50% of gateway pods simultaneously: verify recovery within 60 seconds, error rate <1% during recovery
   - Network partition between gateway and database: verify circuit breaker activates, cached configs continue serving, error rate contained to write operations
   - Database failover (Aurora): verify reconnection within 60 seconds, zero data loss
   - Redis failure (kill all Redis nodes): verify fallback to PostgreSQL, degraded but operational
   - DNS failure (remove geo-DNS record for one region): verify traffic re-routed to surviving regions within DNS TTL (60 seconds)
4. **Recovery metrics per chaos scenario:** time-to-detect (TTD), time-to-recover (TTR), request success rate during event, data loss (must be zero).
5. **Automated weekly run:** Woodpecker CI scheduled pipeline triggers load test + chaos suite against staging every Sunday. Results posted to a Slack channel and stored in S3. Regression detection: alert if p95 latency increases >20% vs previous week.
6. **Game day runbook:** Document procedures for manually running chaos tests in production (with customer notification and reduced blast radius). Plan first production game day within 30 days of Stage 3 launch.
7. **Findings backlog:** Each chaos test failure generates a prioritized bug/improvement ticket. Fix critical findings (data loss, extended outage) before Stage 3 GA.

#### Technical Notes

- **k6 for load testing:** k6 supports WebSocket and SSE protocols natively. Write a custom k6 extension (xk6) for the MCP Streamable HTTP protocol if needed. k6 cloud (Grafana Cloud) can generate distributed load from multiple regions.
- **Chaos tools:** Use Litmus Chaos (Kubernetes-native) or Chaos Mesh for pod-kill and network partition scenarios. For AWS-level chaos (AZ failure, DNS failure), use AWS Fault Injection Simulator (FIS).
- **Staging parity:** Staging must match production topology (multi-region, same auto-scaling policies, same Aurora Global Database setup) for results to be meaningful. Use smaller instance types (cost optimization) but same architecture.
- **Traffic generation:** 100K concurrent SSE connections from a single k6 instance is infeasible (OS limits). Use 10-20 k6 instances (Kubernetes Jobs or EC2 instances) generating 5-10K connections each.
- **Cost:** Load test infrastructure is ephemeral. Estimated cost per run: $50-100 (EC2 spot instances for k6 generators, staging infra running for 2-4 hours).

#### Dependencies

- S-075 (auto-scaling -- must be functional for load tests to exercise it)
- S-077 (multi-region -- chaos tests for DNS failover require multi-region deployment)
- S-083 (observability -- metrics collection required to measure chaos test impact)

---

### S-085: Zero-Downtime Deployments

**Priority:** P0
**Estimated Effort:** 8 story points
**Stage:** Stage 2 (rolling), Stage 3 (blue-green, canary)

#### Description

Implement deployment strategies that guarantee zero downtime for users during gateway updates, database migrations, and configuration changes. At Stage 2, this means rolling deployments with health checks. At Stage 3, add blue-green deployments for major versions, canary deployments for gradual rollout, and feature flags for decoupling deploy from release.

Database migrations are a critical focus: all migrations must be backward-compatible (the old gateway version must work with the new schema and vice versa) using the expand-contract pattern.

#### Acceptance Criteria

1. **Rolling deployment (Stage 2):** ECS rolling update deploys one new task at a time. Old task is drained (30s connection draining) before termination. New task must pass health check within 60 seconds. If health check fails, rollback to previous task definition automatically.
2. **Blue-green deployment (Stage 3):** Two complete environments (blue and green) deployed on EKS. Traffic is routed to the active environment. New version deployed to the inactive environment, verified via smoke tests, then traffic switched via ALB target group swap. Rollback: switch back to the previous environment within 30 seconds.
3. **Canary deployment (Stage 3):** New version receives 5% of traffic initially. If error rate and latency remain within thresholds (error rate <1%, p95 latency <300ms) for 10 minutes, promote to 25%, then 50%, then 100%. Automated promotion with manual override. Automatic rollback if thresholds are breached.
4. **Database migrations:** All migrations are backward-compatible. Pattern: (a) expand: add new columns/tables without removing or renaming existing ones, (b) deploy new code that writes to both old and new schema, (c) backfill: populate new columns from old data, (d) contract: deploy code that reads/writes only new schema, (e) drop old columns/tables in a subsequent migration. Each step is a separate deployment.
5. **Migration safety checks:** CI pipeline validates that every migration file: (a) does not contain `DROP COLUMN`, `DROP TABLE`, `ALTER COLUMN ... TYPE`, or `RENAME` in the expand phase, (b) includes a rollback migration, (c) executes in <10 seconds on a test database with production-scale data.
6. **Feature flags:** LaunchDarkly, Unleash (self-hosted), or a simple database-backed feature flag system. New features deployed behind flags. Flags evaluated per-request with <1ms overhead. Flag changes propagate within 10 seconds.
7. **Deployment pipeline:** Woodpecker CI -> build Docker image -> push to registry -> deploy to staging (rolling) -> automated smoke tests -> manual approval -> deploy to production (rolling/canary). Total pipeline time: <15 minutes.
8. **Rollback verified:** simulate a bad deployment (health check failure). Automatic rollback completes within 2 minutes. No requests lost during rollback. Alert fired to on-call.

#### Technical Notes

- **ECS rolling update:** Set `minimumHealthyPercent: 100` and `maximumPercent: 200`. ECS launches new tasks first, waits for health check, then drains old tasks. This guarantees capacity never drops below current levels.
- **Canary with Istio (Stage 3):** Use Istio VirtualService to split traffic between two Kubernetes Deployments (stable and canary). Flagger (Weaveworks) or Argo Rollouts can automate the progressive promotion based on Prometheus metrics.
- **Expand-contract example:** Adding a `config_version` column to `mcp_servers`: (1) Migration: `ALTER TABLE mcp_servers ADD COLUMN config_version INT DEFAULT 1`. (2) Deploy code that writes `config_version` on every update. (3) Backfill: `UPDATE mcp_servers SET config_version = 1 WHERE config_version IS NULL`. (4) Deploy code that reads `config_version`. (5) Next migration: `ALTER TABLE mcp_servers ALTER COLUMN config_version SET NOT NULL`.
- **Feature flag for database migrations:** Use a feature flag to control which code path (old schema vs new schema) is active. This decouples the deploy (new code is running) from the release (new behavior is active). If the new code path has issues, flip the flag without redeploying.
- **Long-running SSE connections:** Deployments must not kill SSE connections that have been active for hours. Connection draining gives 30 seconds for the client to reconnect. MCP clients must handle reconnection -- verify this in integration tests.

#### Dependencies

- S-075 (auto-scaling -- new instances must be ready before old ones are drained)
- S-076 (load balancer -- connection draining and health check configuration)
- S-083 (observability -- canary promotion relies on metrics)

---

### S-086: Data Sovereignty and Compliance

**Priority:** P2
**Estimated Effort:** 8 story points
**Stage:** Stage 3 (required for EU customers and enterprise sales)

#### Description

Implement data sovereignty controls and compliance mechanisms required for EU customers (GDPR), enterprise sales (SOC 2 preparation), and regional data residency requirements. EU user data must stay within the EU region. Users must be able to export and delete their data. All data access must be auditable.

#### Acceptance Criteria

1. **Data residency:** Users are assigned a home region at signup (based on IP geolocation or explicit selection). All user data (server configs, credentials, audit logs, usage metrics) stored in the home region's Aurora cluster. EU users' data stored exclusively in eu-west-1.
2. **Cross-region data isolation:** A query in us-east-1 for an EU user's data returns no results. Data residency enforced at the application layer (home region tag on all queries) and verified by row-level security policies in PostgreSQL.
3. **GDPR data export:** API endpoint `GET /api/users/{id}/export` returns a JSON archive containing: user profile, all server configurations, audit log entries, and usage metrics. Excludes raw credentials (exported as metadata: auth type, created date, last used date). Export completes within 5 minutes for users with up to 100 servers.
4. **GDPR right to deletion:** API endpoint `DELETE /api/users/{id}` triggers: (a) immediate deactivation of all MCP servers, (b) deletion of all server configs, credentials, and usage data within 30 days, (c) anonymization of audit log entries (replace user_id with a hash), (d) confirmation email. Deletion is irreversible after 30-day grace period.
5. **Data retention policies:** Configurable per region. Defaults: audit logs retained 365 days, request logs retained 90 days, usage metrics retained 24 months. Retention enforcement automated via partition detach + S3 archival (S-073).
6. **Encryption verification:** All data encrypted at rest (Aurora encryption, S3 SSE-KMS) and in transit (TLS 1.2+). Per-region encryption keys (KMS keys created in each region). Encryption status auditable via AWS Config rules.
7. **Audit trail:** All data access (reads and writes) to sensitive tables (credentials, user profiles) logged in the audit_log table with: accessor identity (user, admin, system job), action, resource, timestamp, source IP. Audit logs are immutable (append-only, no UPDATE/DELETE permissions on the audit_log table).
8. **Compliance documentation:** Produce artifacts for SOC 2 readiness: data flow diagrams showing where data resides, encryption inventory, access control matrix, retention policy documentation, incident response procedures.

#### Technical Notes

- **Data residency implementation:** Add a `home_region` column to the `users` table (enum: `us-east-1`, `eu-west-1`, `ap-southeast-1`). All write operations check that the current region matches the user's home region. If not, forward the write to the correct region. Read operations in the wrong region return 404 (data is not replicated to non-home regions for sovereignty-controlled users).
- **Aurora Global Database conflict:** Aurora Global Database replicates ALL data to all regions. For data sovereignty, EU user data must NOT replicate to US. Two approaches: (a) separate Aurora clusters per region (no global replication for user data, only shared reference data), (b) use Aurora Global Database for non-sensitive data and a separate regional cluster for credentials and PII. Approach (a) is simpler but loses cross-region failover. Approach (b) is complex but preserves HA. Document the trade-off and choose based on customer requirements.
- **GDPR deletion and immutable audit logs:** Audit log entries for deleted users are anonymized (user_id replaced with SHA-256 hash), not deleted. This preserves the audit trail for compliance while removing PII. Legal review required to confirm this approach satisfies GDPR Article 17.
- **SOC 2 scope:** SOC 2 Type II requires 6-12 months of evidence collection. This story produces the technical controls; organizational controls (employee background checks, security training, vendor management) are out of scope.

#### Dependencies

- S-077 (multi-region deployment -- regional infrastructure must exist)
- S-078 (Aurora Global Database -- data residency impacts replication strategy)
- S-080 (credential management -- per-region encryption keys)
- S-073 (partitioning -- retention enforcement via partition management)

---

### S-087: API Gateway and Rate Limiting at Scale

**Priority:** P0
**Estimated Effort:** 5 story points
**Stage:** Stage 2

#### Description

Implement distributed rate limiting using Redis that enforces per-server, per-user, and per-IP limits tiered by pricing plan. At Stage 1, rate limiting is in-process (Tower middleware, single instance). At Stage 2+ with multiple instances, rate limits must be globally consistent -- a user cannot bypass limits by having their requests hit different instances.

#### Acceptance Criteria

1. Rate limits enforced per pricing tier:
   - Free: 100 calls/min per server, 500 calls/min per user, 50 calls/min per IP
   - Pro: 1,000 calls/min per server, 5,000 calls/min per user, 500 calls/min per IP
   - Enterprise: 10,000 calls/min per server, 50,000 calls/min per user, 5,000 calls/min per IP
2. Rate limiting algorithm: sliding window (1-minute window) with burst allowance of 150% of the per-minute limit (e.g., free tier allows 150 calls in a burst, then rate-limited until the window slides).
3. Rate limit state stored in Redis. Atomic operations (Lua script) ensure accuracy under concurrent access from multiple gateway instances. Key format: `ratelimit:{scope}:{id}:{window}` with automatic TTL.
4. Rate limit headers included in every response: `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset` (Unix timestamp). Rate-limited responses return HTTP 429 with a JSON body: `{"error": "rate_limit_exceeded", "retry_after_seconds": N}`.
5. Rate limit fallback when Redis is unavailable: in-process token bucket per instance. Effective limits are per-instance (not global) during Redis outage. This is documented as degraded behavior.
6. Rate limit bypass for internal health checks and monitoring endpoints.
7. Rate limit metrics: `gateway_rate_limit_hits_total` (counter, labels: tier, scope), `gateway_rate_limit_remaining` (gauge, sampled), Redis latency for rate limit operations (p50/p95).
8. Rate limit dashboard in Grafana: top 10 rate-limited servers, top 10 rate-limited users, rate limit hit rate by tier, Redis latency for rate limit checks.
9. Admin override: ability to set custom rate limits for specific servers or users (e.g., during a demo or load test). Stored in database, cached in Redis with 60-second TTL.

#### Technical Notes

- **Sliding window in Redis:** Use a sorted set per rate limit key. Each request adds a member with the current timestamp as score. `ZREMRANGEBYSCORE` removes entries outside the window. `ZCARD` counts remaining entries. Wrap in a Lua script for atomicity. This is more accurate than fixed-window counters (no burst-at-boundary problem) but uses more memory (~100 bytes per tracked request per window).
- **Memory estimation:** 100K servers x 100 calls/min average = 10M sorted set entries x ~100 bytes = ~1GB. This fits in a `cache.t4g.medium` (3.09 GB) with headroom. At higher volumes, switch to a probabilistic counter (HyperLogLog) for approximate counting, or use fixed windows with a correction factor.
- **Per-IP rate limiting:** Defense against credential stuffing and abuse from unauthenticated clients. Applied before authentication (at the MCP transport layer). Use `X-Forwarded-For` from ALB (trusted proxy) for the client IP.
- **Tower middleware integration:** The existing Tower rate limiting middleware (Stage 1) can be refactored to use Redis as the backend with minimal API changes. The middleware checks Redis, and if Redis is unavailable, falls back to the in-process token bucket.

#### Dependencies

- S-074 (Redis cache layer -- rate limiting counters stored in Redis)
- S-083 (observability -- metrics and dashboard for rate limiting)

---

### S-088: Queue-Based Config Propagation

**Priority:** P1
**Estimated Effort:** 5 story points
**Stage:** Stage 3 (replace LISTEN/NOTIFY at >5,000 servers)

#### Description

Replace PostgreSQL LISTEN/NOTIFY with SNS/SQS for config change propagation at scale. At Stage 1-2, LISTEN/NOTIFY works well: the database notifies all connected gateway instances of config changes. At Stage 3 with 20-100 pods, LISTEN/NOTIFY has limitations: it requires each pod to maintain a dedicated database connection for listening, messages are lost if a pod is disconnected during a publish, and there is no message durability or replay.

SNS/SQS provides: durable message delivery, per-pod SQS queues (fan-out), ordered delivery per partition key, and dead letter handling.

#### Acceptance Criteria

1. SNS topic created: `config-changes`. Message format: `{server_id, user_id, change_type (create|update|delete), config_version, timestamp}`. Messages published by the Platform API on every config change.
2. Per-pod SQS queue created dynamically when a gateway pod starts. Queue subscribes to the SNS topic with a filter policy matching the pod's config partition (if sharding is used) or no filter (if full replication). Queue deleted on pod termination.
3. Message processing: gateway pod polls its SQS queue. On receiving a config change message: (a) fetch updated config from PostgreSQL (source of truth), (b) update in-memory cache, (c) invalidate Redis cache entry (S-074), (d) acknowledge the SQS message.
4. Ordered delivery per server_id: SNS FIFO topic with message group ID = `server_id`. This ensures config changes for the same server are processed in order. Prevents race conditions (e.g., create then immediately update).
5. At-least-once delivery: cache updates must be idempotent. Processing the same message twice (config version N applied twice) produces identical state. Use `config_version` to detect and skip stale messages.
6. Fallback: periodic full cache refresh every 5 minutes. If SQS delivery fails (queue backlog, processing errors), the periodic refresh catches up. This is the safety net -- the system is eventually consistent even if SQS fails entirely.
7. Message propagation latency: config change reflected in all gateway pods within 5 seconds under normal conditions. Measured end-to-end: Platform API publish -> SQS delivery -> cache update -> request serves new config.
8. SQS dead letter queue: messages that fail processing 3 times are moved to DLQ. Alert on DLQ depth > 0. Manual investigation required (likely a bug in the message handler or a corrupt config).
9. Monitoring: messages published per minute, messages processed per minute (per pod), processing latency (p50/p95), DLQ depth, message age (oldest unprocessed message).

#### Technical Notes

- **SNS FIFO vs standard:** FIFO topics guarantee ordering per message group ID and exactly-once delivery. Cost: $0.50 per million publishes (5x standard). At 1,000 config changes/day, cost is negligible. FIFO is worth it for ordering guarantees.
- **Dynamic SQS queues:** Each pod creates a queue on startup (name: `config-changes-{pod-id}`) and subscribes it to the SNS topic. On graceful shutdown, the pod deletes the queue. On ungraceful shutdown (OOM kill, spot interruption), the queue accumulates messages. Use SQS message retention (4 days) and a cleanup job that deletes orphaned queues (no consumer for >1 hour).
- **PostgreSQL LISTEN/NOTIFY coexistence:** During migration, run both LISTEN/NOTIFY and SNS/SQS. The cache update handler is idempotent, so receiving the same change from both channels is safe. Remove LISTEN/NOTIFY after validating SNS/SQS reliability for 2 weeks.
- **Cross-region:** SNS does not natively replicate across regions. For multi-region (S-077), publish config changes to a regional SNS topic in each region. The Platform API (which processes writes in the primary region) publishes to all regional SNS topics. Alternatively, use EventBridge for cross-region event routing.

#### Dependencies

- S-074 (Redis -- cache invalidation on message receipt)
- S-072 (config sharding -- determines SQS filter policy)
- S-077 (multi-region -- cross-region propagation)

---

### S-089: Tenant Isolation and Noisy Neighbor Protection

**Priority:** P0
**Estimated Effort:** 8 story points
**Stage:** Stage 2

#### Description

Implement resource isolation and protection mechanisms that prevent a single MCP server (or user) from degrading the platform for other tenants. In a multiplexed gateway where all servers share the same process, a single server making expensive upstream API calls, receiving large responses, or attracting disproportionate traffic can exhaust shared resources (connections, memory, CPU).

This story implements per-server resource limits, per-upstream circuit breakers, timeout budget allocation, and priority queuing for paid tiers.

#### Acceptance Criteria

1. **Per-server connection limit:** Maximum 50 concurrent MCP client connections per server (free tier), 200 (pro), 500 (enterprise). Connections beyond the limit receive HTTP 503 with `Retry-After` header. Limit enforced in the gateway before any processing.
2. **Per-server request rate limit:** Maximum 100 requests/min (free), 1,000 (pro), 10,000 (enterprise). This is the same rate limiting from S-087 but enforced at the connection admission layer (before credential injection), not just at the response layer.
3. **Response size limit:** Maximum 100KB response from upstream API. Responses exceeding the limit are truncated with an error appended: `{"error": "response_truncated", "message": "Upstream response exceeded 100KB limit"}`. Configurable per tier (enterprise: 1MB).
4. **Upstream timeout:** Maximum 30 seconds (default, configurable per server up to 60 seconds for paid tiers). Timeout starts when the gateway sends the request to the upstream API. On timeout, return error to MCP client and log the event.
5. **Circuit breaker per upstream API:** Track error rate per upstream URL (grouped by host). If error rate exceeds 50% over a 60-second window (minimum 10 requests), open the circuit for 30 seconds. During open state, requests return immediately with an error (no upstream call). Half-open state: allow 1 request through every 10 seconds. If successful, close the circuit. Circuit breaker state stored in Redis (shared across instances).
6. **Timeout budget allocation:** Total request timeout (from MCP client perspective) is partitioned: config lookup (5ms budget), credential injection (10ms budget), upstream call (30s budget, configurable), response transformation (50ms budget). If any stage exceeds its budget, the request is terminated early with a descriptive error.
7. **Priority queuing:** When the gateway is under load (active connections > 70% capacity), requests from paid tiers are prioritized over free tier. Implementation: two internal queues (priority and standard). Paid-tier requests enter the priority queue. Free-tier requests enter the standard queue. Priority queue processed first. Starvation prevention: standard queue guaranteed at least 20% of capacity.
8. **Noisy neighbor detection:** Alert when a single server or user consumes >10% of total gateway capacity (connections or request rate). Dashboard showing top 10 resource consumers.
9. **Per-server metrics isolation:** Each server's resource usage tracked independently. Dashboard shows: connection count, request rate, upstream error rate, average response size, circuit breaker state.

#### Technical Notes

- **Connection limiting:** Use an atomic counter in Redis (key: `connections:{server_id}`, value: current count). Increment on connection open, decrement on connection close. Use a Lua script for atomic check-and-increment. Handle counter drift (pod crash without decrement) via periodic reconciliation (every 60 seconds, count actual connections and correct Redis).
- **Circuit breaker implementation:** Use a sliding window error counter in Redis (similar to rate limiting). Key: `circuit:{upstream_host}`, value: sorted set of `{timestamp, success/failure}`. Check error rate before each upstream call. State transitions (closed -> open -> half-open -> closed) managed by the gateway instance making the request. Race conditions between instances are acceptable (worst case: a few extra requests during state transition).
- **Priority queuing in Rust:** Use `tokio::sync::Semaphore` with two semaphores: `priority_semaphore` (80% of max concurrent requests) and `standard_semaphore` (20%). Paid-tier requests acquire from `priority_semaphore` (falling back to `standard_semaphore` if priority is exhausted). Free-tier requests acquire from `standard_semaphore` only. This prevents starvation while giving priority to paid tiers.
- **Timeout budget:** Implement as a `tokio::time::timeout` wrapper at each stage of request processing. Pass a remaining budget through the request context. Each stage deducts its allocation. If the budget is exhausted, the stage is skipped or returns an error.
- **Response size limit:** Read the upstream response body in chunks. If accumulated size exceeds the limit, stop reading, close the upstream connection, and return the truncated response. Do not buffer the entire response in memory before checking size.

#### Dependencies

- S-074 (Redis -- connection counters and circuit breaker state)
- S-087 (rate limiting -- shared infrastructure for per-server and per-user limits)
- S-083 (observability -- per-server metrics and noisy neighbor detection alerts)

---

## Story Dependency Graph

```
S-070 (Stateless Verification)
  |
  +---> S-071 (Connection Pooling)
  |       |
  |       +---> S-072 (Config Sharding)
  |       |       |
  |       |       +---> S-088 (Queue-Based Config Propagation)
  |       |
  |       +---> S-073 (Database Partitioning)
  |       |
  |       +---> S-080 (Credential Management)
  |       |       |
  |       |       +---> S-081 (Async Job Queue)
  |       |               |
  |       |               +---> S-082 (Usage Metering)
  |       |
  |       +---> S-074 (Redis Cache Layer)
  |               |
  |               +---> S-087 (Rate Limiting)
  |               |
  |               +---> S-089 (Tenant Isolation)
  |               |
  |               +---> S-081 (Async Job Queue)
  |
  +---> S-075 (Auto-Scaling)
  |       |
  |       +---> S-076 (Load Balancer)
  |       |       |
  |       |       +---> S-079 (CDN & Edge Caching)
  |       |
  |       +---> S-077 (Multi-Region)
  |               |
  |               +---> S-078 (Aurora Global Database)
  |               |
  |               +---> S-086 (Data Sovereignty)
  |               |
  |               +---> S-084 (Chaos Engineering)
  |
  +---> S-085 (Zero-Downtime Deployments)
  |
  +---> S-083 (Observability)
```

## Priority Summary

| Priority | Stories | Rationale |
|----------|---------|-----------|
| **P0** | S-070, S-071, S-074, S-075, S-085, S-087, S-089 | Foundational for Stage 2 launch. Without these, horizontal scaling does not work safely. |
| **P1** | S-072, S-073, S-076, S-078, S-079, S-080, S-081, S-083, S-088 | Required for Stage 2 maturity and Stage 3 preparation. Can be delivered incrementally. |
| **P2** | S-077, S-082, S-084, S-086 | Stage 3 features. High effort, high value, but not blocking until 10K+ servers. |

## Effort Summary

| Story | Points | Stage |
|-------|--------|-------|
| S-070 | 3 | Pre-Stage 2 |
| S-071 | 5 | Stage 2 |
| S-072 | 8 | Stage 2 |
| S-073 | 5 | Stage 2-3 |
| S-074 | 8 | Stage 2 |
| S-075 | 8 | Stage 2-3 |
| S-076 | 5 | Stage 2 |
| S-077 | 13 | Stage 3 |
| S-078 | 8 | Stage 3 |
| S-079 | 5 | Stage 2-3 |
| S-080 | 8 | Stage 2-3 |
| S-081 | 5 | Stage 2 |
| S-082 | 8 | Stage 2-3 |
| S-083 | 8 | Stage 2-3 |
| S-084 | 8 | Stage 3 |
| S-085 | 8 | Stage 2-3 |
| S-086 | 8 | Stage 3 |
| S-087 | 5 | Stage 2 |
| S-088 | 5 | Stage 3 |
| S-089 | 8 | Stage 2 |
| **Total** | **133** | |
