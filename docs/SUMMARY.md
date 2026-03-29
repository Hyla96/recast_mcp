# Dynamic MCP Server Builder — Full Context

---

## 1. Product Vision

Recast MCP is a hosted, visual, no-code platform that exposes any REST API to AI agents (Claude, Cursor, ChatGPT) as a live MCP server in 90 seconds — no code, no DevOps, no credential management drama. Tagline: "The fastest way to expose any REST API to AI agents." Strategic framing: "Vercel for MCP servers." Core bet: OSS-first distribution, monetized through hosted convenience.

---

## 2. Core Value Proposition

- Paste a URL, configure auth (3 clicks), click response fields to map them, deploy — 90 seconds end-to-end
- Document renderer turns raw JSON into human-readable cards; field selection infers JSONPath automatically
- Security by default: AES-256-GCM credential encryption, credential injector sidecar (gateway never holds raw credentials), SSRF blocklist, audit logging — all on from day one
- Hot reload: config save to live server in under 2 seconds (no build pipeline)
- Fills the unoccupied quadrant: hosted + any REST API + no-code + managed multi-tenant auth
- Export to Docker for self-hosted/on-prem use (v1.1)

---

## 3. Target Users

- **Week-1 customer:** Backend developer whose non-catalog API isn't in Composio/Pipedream, found via HN/Product Hunt
- **Primary paying segment:** AI consultants and freelancers (60-70% conversion probability, 8-month LTV, recurring per-engagement need)
- **Secondary:** API-first startups and mid-market platform teams without existing API gateway investments
- **Excluded Year 1:** Enterprise on Kong/Azure APIM; non-technical users (no-code promise breaks on complex APIs before template library is mature)

---

## 4. Key Product Decisions

- **Architecture: Option B (Gateway)** — single shared Rust proxy, config-driven routing, instant deploy, PostgreSQL LISTEN/NOTIFY hot reload. NOT code generation as primary serving path.
- **Credential injector sidecar in MVP** (not v2) — separate process, sole credential access, gateway sends request skeletons only. Non-negotiable.
- **Primary UX: Click-to-Select** — document renderer + click on values. LLM Smart Fill as secondary button, ships Week 6 behind feature flag. Drag-and-drop mapper deferred to v2.
- **MCP protocol scope for MVP:** `tools/list`, `tools/call`, `initialize`, `initialized` only. Resources, Prompts, Sampling, Roots deferred.
- **Transport:** Streamable HTTP primary; SSE required fallback (Claude Desktop compatibility). Verify Claude Desktop + Streamable HTTP in Week 2 before proceeding.
- **Auth in MVP:** Bearer Token, API Key (header/query), Basic Auth. OAuth 2.0, mTLS, HMAC signing deferred.
- **Transforms:** Declarative only (JSONPath, field rename, safe arithmetic, array flattening). No Turing-complete scripting in the shared proxy.
- **OSS-first:** Core product open source on day one. Monetize via hosted convenience.
- **No VC funding.** Nights-and-weekends months 0-6; part-time bootstrap months 6-12; full-time only at $10K+ MRR.
- **5 launch demo APIs:** Stripe, GitHub, OpenWeather, Hacker News, Salesforce.

---

## 5. System Architecture

**Selected: Option B — The Gateway (shared multi-tenant proxy).** Single Rust process serves all user-created MCP servers through config-driven routing. No per-user containers.

```
MCP Client --> gateway.example.com/mcp/{server_id}
                  |
              [Rust axum Router]
                  | (cache miss: PostgreSQL; cache hit: in-memory moka HashMap)
              [In-memory Config Cache]  <-- LISTEN/NOTIFY from PostgreSQL
                  |
              [Request Builder]  -- URL interpolation, auth header injection
                  |
              [Credential Injector Sidecar]  -- decrypts + injects credentials
                  |                             (separate process; gateway never
              [Upstream HTTP Client]             holds raw credentials)
                  |
              [Response Transformer]  -- JSONPath extraction, safe arithmetic,
                  |                     array flattening (declarative only)
              [MCP Serializer]  -- JSON-RPC 2.0 over Streamable HTTP (primary)
                                   or SSE (fallback)

Control Plane (separate Rust axum service):
  Platform API <--> PostgreSQL <--> Clerk (user auth)
  Builder UI (React/Vite) --> Platform API --> writes config --> NOTIFY fires
```

**Hot reload:** PostgreSQL `LISTEN/NOTIFY`. Config write triggers notification; gateway reloads affected server config within <2s. No process restart.

**MCP protocol surface (MVP):** `tools/list` + `tools/call` only. Streamable HTTP primary; SSE fallback. JSON-RPC 2.0 framing.

---

## 6. Tech Stack

| Layer                  | Choice                                                                                            |
| ---------------------- | ------------------------------------------------------------------------------------------------- |
| Gateway + Platform API | Rust, axum, tokio, sqlx, serde, jsonpath-rust, aes-gcm, reqwest, tower                            |
| Frontend               | React 19 + TypeScript, Vite, Zustand, @dnd-kit, @tanstack/virtual, jsonpath-plus, fast-xml-parser |
| Database               | PostgreSQL (JSONB configs, pgcrypto, LISTEN/NOTIFY)                                               |
| Auth (platform)        | Clerk (React + Rust SDKs, 10K MAU free)                                                           |
| DNS/TLS/CDN            | Cloudflare (free tier at launch)                                                                  |
| CI/CD                  | Woodpecker CI + Docker                                                                            |
| Export                 | Handlebars templates -> FastMCP (Python)                                                          |

---

## 7. Data Model

```sql
users          (id, email, hashed_password, plan, created_at)

mcp_servers    (id, user_id, name, slug, config_json JSONB,
                status, created_at, updated_at)
                -- config_json: tool defs, upstream URLs, auth type, field mappings, rate limits

credentials    (id, server_id, auth_type, encrypted_payload BYTEA,
                iv BYTEA, created_at)
                -- AES-256-GCM; per-row IV; key in env var (launch), per-user DEK via KMS (growth)

request_logs   (id, server_id, method, upstream_url, status_code,
                latency_ms, created_at)

audit_log      (id, actor_id, action, resource_id, metadata JSONB, created_at)
                -- immutable, append-only. Non-negotiable MVP.
```

---

## 8. Infrastructure Tiers

| Tier      | Target          | Stack                                                                    | Cost           |
| --------- | --------------- | ------------------------------------------------------------------------ | -------------- |
| Bootstrap | 0-500 servers   | Railway/Fly.io, Railway PostgreSQL, Cloudflare free                      | ~$21/month     |
| Growth    | 500-10K servers | ECS Fargate, RDS Multi-AZ, ALB, ElastiCache Redis, KMS                   | ~$724/month    |
| Scale     | 10K-100K+       | EKS multi-region, Aurora Global DB, Redis Cluster, Vault, Istio, Datadog | ~$6,700+/month |

**Migration triggers:** 500 active servers OR $5K MRR → Growth tier. Scale-to-zero not applicable (SSE requires persistent connections).

---

## 9. Security Model

| Control               | Implementation                                                                                                           |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Encryption at rest    | AES-256-GCM, per-row IV, env var key (launch) / KMS DEK (growth)                                                         |
| Encryption in transit | TLS 1.3 at Cloudflare edge                                                                                               |
| Credential injection  | Separate sidecar process; gateway sends request skeletons over Unix domain socket, sidecar decrypts + injects + forwards |
| Credential logging    | Redaction filter on all log paths. Never logged.                                                                         |
| SSRF protection       | Blocklist + post-DNS IP validation. Blocks RFC 1918, link-local, 169.254.169.254. Hard gate.                             |
| MCP client auth       | Per-server Bearer tokens, revocable                                                                                      |
| Platform auth         | Clerk sessions (short TTL). Clerk outage doesn't affect live MCP servers.                                                |
| Audit logging         | All credential access, auth failures, SSRF blocks, admin actions. Non-negotiable MVP.                                    |
| DB isolation          | Row-level security policies (growth); application-level WHERE (launch)                                                   |

---

## 10. Performance Targets

- Gateway overhead (p95): < 200ms (excluding upstream)
- Hot reload propagation: < 2 seconds
- `tools/list` response (p95): < 50ms
- Concurrent MCP sessions per instance: 500+
- Config cache lookup: < 1 µs (moka)
- JSON-RPC parse: < 10 µs

---

## 11. Hard Constraints (MVP)

- JSON responses only (XML Week 6 if time; HTML/binary post-MVP)
- Response size limit: 100KB (truncated with warning; pagination v2)
- Response timeout: 30 seconds
- Public internet APIs only (SSRF blocklist enforced)
- Rate limits: 100 calls/min per server, 1,000 calls/min per user (token bucket via Tower)
- No streaming responses; no conditional logic transforms; no multi-step chaining

---

## 12. UX Design

**Core flow (6 steps):**

1. Name the server
2. Paste REST endpoint URL + method selector; auto-detects path/query params
3. Configure auth: None / API Key / Bearer / Basic
4. Run test call; response renders as human-readable document card (not raw JSON)
5. Map fields via click-to-select on rendered document; LLM Smart Fill as secondary action
6. Review summary → Deploy; success screen provides copy-paste config blocks for Claude Desktop, Cursor, VS Code

**Key screens:** Dashboard, Builder drawer, Document Renderer + field mapper (two-panel), Review overlay, Success + Playground panel.

**Playground:** Post-deploy tool invocation panel; accepts params, fires real tool call, returns JSON.

**Edge cases:** Internal URLs → "paste sample response" fallback. Arrays → modal with 4 inclusion options. XML → silent JSON conversion with banner.

---

## 13. UX Critical Moments

1. **Test call failure** — raw HTTP codes cause abandonment. Every error needs plain-English diagnosis + concrete next step.
2. **Document renderer** — must promote key fields (name/status/id/email) to card header, collapse arrays/nested by default, format timestamps.
3. **Tool description field** — users who write "gets a customer" deploy unusable tools. Needs prominent helper text + example + optional LLM draft.
4. **Success/config screen** — gap between "no-code promise" and JSON copy-paste handoff to Claude Desktop config.
5. **Internal API blocker** — "paste sample response" escape hatch saves the developer segment.

---

## 14. Competitive Landscape

| Competitor                    | Position                                     | Gap                                            |
| ----------------------------- | -------------------------------------------- | ---------------------------------------------- |
| Composio, Pipedream, Latenode | Hosted + fixed catalog (500-3K integrations) | No arbitrary API support                       |
| Speakeasy, Stainless, Fern    | Code output + fixed catalog                  | Requires OpenAPI spec; user deploys            |
| FastMCP, api-wrapper-mcp      | Self-deploy + any API                        | Zero UI, zero hosting                          |
| Alpic ($6M), Manufact ($6.3M) | Deployment infra for devs                    | Not a no-code builder                          |
| Kong, Azure APIM              | Enterprise gateway                           | MCP export shipping Q4 2025; enterprise-priced |

**Our quadrant (Hosted + Any API + No-Code + Managed Auth) is unoccupied.** Real competition: developer willingness to spend 4-8 hours with FastMCP; Anthropic's forthcoming native builder (65% probability in 9 months).

---

## 15. Pricing

| Tier       | Price                            | Included                                  |
| ---------- | -------------------------------- | ----------------------------------------- |
| Community  | Free                             | Self-hosted OSS                           |
| Pro        | $19/month + $0.002/call over 50K | 1 server, visual builder, managed hosting |
| Team       | $79/month                        | 5 servers, shared workspaces, audit logs  |
| Enterprise | $5,000-$10,000/year              | Unlimited, on-prem, SOC 2, SAML           |

---

## 16. Financial Model

- Year 1: ~$10,600 ARR, ~$21,800 net loss
- Year 2: ~$70,500 ARR, ~$25,500 profit
- Break-even: Month 20-22
- $100K ARR path: 400 Pro + 50 Team + 2 Enterprise by Month 20-24
- No VC. ~$5K cash + $78K opportunity cost months 0-12.
- Exit scenarios: Lifestyle ($50-100K ARR, 50%), acquisition ($5-20M, 30%), pivot/abandon (20%).

---

## 17. Go-to-Market

- Month 0-1: Show HN + Product Hunt. Target: 1,000 GitHub stars, 500 signups.
- Month 1-3: Content-led ("State of MCP Server Security"), tutorial series (Stripe/GitHub/Salesforce in 5 min), outreach to 200 API-first companies.
- Month 3-5: Freemium mechanics, in-product upgrade triggers. Target: $2K-3K MRR.
- Month 6-12 (gated on $5K+ MRR): Enterprise entry, SOC 2 pursuit.

---

## 18. Risks & Open Questions

### Strategic (CRITICAL)

- **SR-1:** Anthropic ships native builder (65% / 9mo) — mitigation: multi-client, OSS community moat, acquisition target
- **SR-2:** Alpic/Manufact ships same product faster (55%) — mitigation: 8-week timeline advantage
- **SR-4:** "Feature not product" (80%) — mitigation: template library + multi-tenant auth + observability create platform layer

### Product (HIGH)

- **PR-1:** Click-to-Select fails on complex APIs (60%) — gate at Week 4; fallback: JSON tree checkboxes
- **PR-3:** MCP transport incompatibility with Claude Desktop (30%) — HARD GATE at Week 2

### Business (HIGH)

- **BR-1:** MCP servers built once, customers churn after 3 months (70%) — target consultants for recurring use
- **BR-3:** Credential trust barrier without SOC 2 (60%) — sidecar + encryption messaging; SOC 2 at $10K MRR

### Technical (MEDIUM-HIGH)

- **TR-1:** No mature Rust MCP server library (40%) — mid-Week 2 checkpoint; language switch trigger
- **TR-2:** Upstream API diversity unbounded (80%) — scope constraints are product features

### Unresolved ADRs

- ADR-001: MCP transport priority (Streamable HTTP vs SSE vs both)
- ADR-002: Credential injector architecture (HTTP sidecar vs gRPC vs in-process)
- ADR-003: Hot reload (LISTEN/NOTIFY vs polling fallback)
- ADR-004: Rate limiting scope (per-server vs per-user vs both)

---

## 19. Grill Report — Top Contradictions

1. **"No competitor" vs. 27 listed competitors.** Addressable market is APIs not in Composio's 3K catalog, without OpenAPI spec, not on Azure, where user won't use OSS CLI. Very small intersection.
2. **Business model doesn't work at team's own numbers.** Year 1: $0 revenue. Year 2 assumes 100 Pro conversions requiring ~10K signups.
3. **"OSS-first" + "Export to Docker" = self-defeating SaaS.** Every exported server is a churned customer.
4. **MCP servers are built once, not used daily.** No recurring engagement driver exists.
5. **Credential trust is the primary adoption barrier.** Why give Stripe secret key to a 2-person startup with no SOC 2?
6. **Window already partially closed.** Azure APIM MCP export shipping. VS Code has "Create MCP Server" templates.
7. **Rust is resume-driven.** MVP target is 50 concurrent connections. Go/TypeScript ships 2-3x faster.
8. **Click-to-select breaks on enterprise APIs** (Salesforce: 200 fields, 5 levels deep). LLM Smart Fill should be primary, not secondary.
9. **Target customer #1 (API-first companies) already has OpenAPI specs** — exactly what Speakeasy/Azure handle.
10. **Security engineer's warning overruled.** Shared credential model called "business-ending event" risk; team proceeded anyway.

**Grill bottom line:** "This is a feature, not a product." Best realistic outcome: well-regarded OSS tool with 2K-5K GitHub stars and acquisition optionality.

---

## 20. [DEBATE] Sections (Preserved)

**[DEBATE] Architecture Selection: Gateway (B) vs. Edge Fabric (C)**

> "Build Option B (The Gateway) if the team wants the fastest path to a working product, plans to validate demand before investing in infrastructure complexity, and is comfortable with the shared-security-boundary tradeoff in the short term. This is the pragmatic MVP choice. Add credential isolation as a separate microservice in v2."
>
> "Build Option C (The Edge Fabric) if the team is comfortable with Cloudflare lock-in, wants the cheapest possible infrastructure, and is confident that MCP's Streamable HTTP transport will be the dominant transport."
>
> Hybrid path selected: Option B for hosted SaaS + Option A (Compiler) as Docker export feature.

**[DEBATE] Credential Injector Sidecar — MVP or v2?**

Architecture doc recommends v2. Feasibility assessment overrides: "Credential injector sidecar — per the resolved debate; shared-memory credentials are business-ending risk." **Resolution: MVP.**

---

## 21. Engineering Roadmap

### Epic Overview

| Epic                    | Stories | Points  | Delivers                                                                                                                                                                              |
| ----------------------- | ------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 00: Project Setup       | 10      | 38      | Cargo+pnpm monorepo, CI, Docker Compose, PostgreSQL schema, shared libs, telemetry, health checks                                                                                     |
| 01: Foundation Services | 10      | 47      | Platform API + Gateway + Credential Injector skeletons, Clerk auth, AES-256-GCM encryption, SSRF, audit log, CRUD, rate limiting                                                      |
| 02: Gateway Core        | 16      | 80      | JSON-RPC 2.0, Streamable HTTP + SSE, config router, moka cache, hot reload, upstream client, credential injection flow, transforms, circuit breaker                                   |
| 03: Builder UI          | 12      | 68      | React SPA: app shell, Clerk auth, dashboard, URL input, auth config, test call, document renderer, click-to-select mapping, array normalization, tool naming                          |
| 04: Deployment & Ops    | 10      | 50      | Deploy flow, connection configs, playground, server detail/status, Bearer token management, deletion, responsive design, accessibility                                                |
| 05: Scalability         | 20      | 133     | Stateless design, connection pooling, config sharding, DB partitioning, Redis, auto-scaling, multi-region, Aurora, CDN, KMS, async jobs, metering, observability, chaos engineering   |
| 06: Advanced Features   | 12      | 80      | LLM Smart Fill, XML conversion, Docker export, template library, multi-tool servers, OAuth 2.0, team workspaces, webhooks, custom domains, versioning, analytics, credential rotation |
| **Total**               | **90**  | **496** |                                                                                                                                                                                       |

### Critical Path

```
EPIC-00 (all) → EPIC-01 (all) → EPIC-02 ──┬── EPIC-03 (sequential internally)
                                             │        ↓
                                             │   EPIC-04
                                             └── EPIC-05 (staged by traffic milestone)
                                                      ↓
                                                 EPIC-06
```

### 15 Most Architecturally Significant Stories

| ID    | Title                          | Pts | Why                                                                   |
| ----- | ------------------------------ | --- | --------------------------------------------------------------------- |
| S-000 | Monorepo Scaffolding           | 3   | Locks Cargo+pnpm workspace layout; all CI/Docker references           |
| S-003 | PostgreSQL Schema + Migrations | 5   | Forward-only sqlx baseline; hot-reload trigger depends on it          |
| S-010 | Platform API Scaffolding       | 5   | 10-layer axum middleware stack inherited by all endpoints             |
| S-013 | Credential Encryption Module   | 8   | AES-256-GCM envelope encryption; security contract for platform       |
| S-014 | SSRF Protection                | 5   | Blocks private IPs, cloud metadata before upstream calls              |
| S-020 | JSON-RPC 2.0 Parser            | 3   | Every MCP message passes through; <10µs parse target                  |
| S-021 | Streamable HTTP Transport      | 5   | Primary MCP transport; handles sync + SSE upgrade on single POST      |
| S-024 | In-Memory Config Cache         | 5   | moka cache, <1µs lookup, 100K configs in 5s startup                   |
| S-025 | Hot Reload (LISTEN/NOTIFY)     | 5   | Config propagation to all instances within 2s, no restart             |
| S-027 | Credential Injection Flow      | 8   | Sidecar protocol over Unix socket; the security isolation boundary    |
| S-029 | Tool Schema Generator          | 3   | Produces MCP tools/list schema; determines tool usability             |
| S-043 | URL Input + Param Detection    | 8   | Auto-detects path/query params; entry point of 90-second promise      |
| S-048 | Click-to-Select Field Mapping  | 8   | Core product differentiator; auto-generates JSONPath from clicks      |
| S-070 | Stateless Design Verification  | 3   | Certifies all in-process state is rebuildable; gates scaling          |
| S-074 | Redis Cache Layer              | 8   | Distributed rate limiting, cross-instance invalidation; gates Stage 2 |

### 8-Week Build Sequence

- **Week 1:** Foundation (Rust scaffold, PostgreSQL schema, Clerk, credential encryption, SSRF, audit log)
- **Week 2:** Gateway core (MCP router, JSON-RPC 2.0, tools/list + tools/call, config cache, LISTEN/NOTIFY, credential sidecar) — **HIGH RISK; if slip > 1 week, re-evaluate Rust vs Go/TypeScript**
- **Week 3:** Document renderer + click-to-select + JSONPath inference
- **Week 4:** Array normalization, tool naming/description, MCP schema generation
- **Week 5:** One-click deploy, playground, per-server Bearer token, Claude Desktop config block
- **Week 6:** LLM Smart Fill (feature flag), XML conversion, rate limiting, error handling
- **Weeks 7-8:** E2E testing with real APIs, security audit, performance benchmarks, launch

### Load-Bearing Features (Cannot Be Cut)

MCP protocol, test-call proxy, click-to-select, credential encryption, SSRF blocklist, audit logging, credential sidecar, per-server Bearer token, hot reload.

---

## 22. User Stories by Persona

**Backend Developer:** Server creation (URL paste, param detection, HTTP method override, unreachable API escape hatch), auth configuration (Bearer/API Key/Basic), test calls with actionable error messages.

**Non-technical / Consultant:** Click-to-select field mapping, array normalization, field rename/remove, tool naming with quality guidance, preview tool schema.

**Platform / Team Lead:** One-click deploy, copy-paste connection configs, Bearer token generation/revocation, deployment management.

**Server Operator:** Dashboard (status/last-call/error-rate), edit/delete server config, audit trail, credential rotation, SSRF notifications.

**Power User / Enterprise:** Docker export, FastMCP project export.

**New User:** Interactive first-run wizard (URL → auth → test → map → deploy), empty state guidance, contextual error recovery.

All P0 stories are launch-blocking. P1 (SSE fallback, POST body builder, Docker export) ships weeks 5-8 or v1.1. P2 (OAuth, drag-and-drop, SAML, multi-API) is post-MVP.
