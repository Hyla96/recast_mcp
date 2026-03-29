# Dynamic MCP Server Builder — Stories Index & Implementation Roadmap

**Date:** March 28, 2026

**Purpose:** Comprehensive planning and overview document for all user stories across all seven epics. Serves as the master project plan for engineering teams, project managers, and stakeholders. Includes story breakdown, dependency mapping, phased rollout, team allocation, and critical path analysis.

---

## Overview

**Product:** Dynamic MCP Server Builder — A no-code platform for creating MCP servers from REST APIs.

**Story Approach:** Stories are organized into 7 epics, each representing a major functional area:
- **Epic 00-05:** MVP (Weeks 1-8) — foundation, core product, and deployment
- **Epic 06:** Growth Stack (Weeks 9-16) — advanced features for differentiation and enterprise adoption

**Story Naming Convention:**
- Epic 00: S-001 to S-015 (infrastructure, project setup)
- Epic 01: S-016 to S-040 (server creation and URL handling)
- Epic 02: S-041 to S-080 (authentication config)
- Epic 03: S-081 to S-100 (test calls and responses)
- Epic 04: S-101 to S-140 (field mapping and click-to-select)
- Epic 05: S-141 to S-180 (deployment, operations, and management)
- Epic 06: S-090 to S-101 (advanced features, post-MVP)

**Total Stories:** 101

**Total Story Points:** ~250-280 (MVP: ~170-190 points; Growth: ~80 points)

**Recommended Team:** 4-6 engineers (2-3 backend, 2 frontend, 1 DevOps/infra)

---

## Epic Summary Table

All 7 epics with counts, points, priority, and duration estimates:

| Epic | Name | Story Count | Est. Story Points | Priority Tier | Target Timeline | Key Deliverables |
|------|------|-------------|-------------------|----------------|-----------------|------------------|
| **00** | Foundation & Infra | 15 | 35 | P0 | Weeks 1-2 | CI/CD, k8s, auth system, secrets mgmt |
| **01** | Server Creation (Core) | 25 | 45 | P0 | Weeks 2-4 | URL parser, param detection, method selector, sample response, auth detection |
| **02** | Authentication Config | 40 | 50 | P0 | Weeks 3-5 | Bearer, API Key, Basic auth, OAuth detection, validation |
| **03** | Test Calls & Responses | 20 | 35 | P0 | Weeks 4-6 | Test execution, response rendering, JSON/XML parsing, error handling |
| **04** | Field Mapping | 40 | 40 | P0 | Weeks 5-8 | Click-to-select UI, JSONPath extraction, field config, tool definition |
| **05** | Deployment & Ops | 40 | 45 | P0 | Weeks 6-10 | Server generation, FastMCP integration, playground, dashboard, management |
| **06** | Advanced Features | 12 | 80 | P1 | Weeks 9-16 | LLM smart fill, XML, Docker export, templates, OAuth, multi-tool, team workspaces, analytics, versioning |

**MVP Total (Epics 00-05):** 180 stories (estimated), ~240 points, 8 weeks

**Growth Stack (Epic 06):** 12 stories, ~80 points, 8 weeks (parallel with final MVP polish)

---

## Dependency Graph (ASCII)

```
┌─────────────────────────────────────────────────────────────────┐
│                      EPIC 00: FOUNDATION                         │
│  (Infrastructure, auth, secrets, CI/CD, monitoring)             │
│                    (Weeks 1-2, 35 points)                        │
│  ├─ Kubernetes cluster setup                                     │
│  ├─ Auth system (login, RBAC)                                    │
│  ├─ Secrets management (env vars, encryption keys)               │
│  ├─ CI/CD pipeline (Woodpecker CI)                               │
│  ├─ Monitoring & logging (Datadog/Prometheus)                    │
│  └─ Database schema (PostgreSQL, migrations)                     │
└─────────────┬───────────────────────────────────────────────────┘
              │
              ├─────────────┬──────────────┬──────────────┬─────────────┐
              │             │              │              │             │
              ▼             ▼              ▼              ▼             ▼
        ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
        │ EPIC 01  │  │ EPIC 02  │  │ EPIC 03  │  │ EPIC 04  │  │ EPIC 05  │
        │ Core     │  │ Auth     │  │ Testing  │  │ Field    │  │ Deploy & │
        │ Server   │  │ Config   │  │ & Resp   │  │ Mapping  │  │ Mgmt     │
        │(25 str,  │  │(40 str,  │  │(20 str,  │  │(40 str,  │  │(40 str,  │
        │ 45 pts)  │  │ 50 pts)  │  │ 35 pts)  │  │ 40 pts)  │  │ 45 pts)  │
        │ W2-4     │  │ W3-5     │  │ W4-6     │  │ W5-8     │  │ W6-10    │
        └──────────┘  └──────────┘  └──────────┘  └──────────┘  └──────────┘
              │             │              │              │             │
              └─────────────┴──────────────┴──────────────┴─────────────┘
                                           │
                                           ▼
                      ┌──────────────────────────────────────┐
                      │   EPIC 06: ADVANCED FEATURES         │
                      │   (Growth Stack, Post-MVP)           │
                      │   (12 stories, ~80 points)           │
                      │   (Weeks 9-16, parallel with MVP)    │
                      │   ├─ S-090: LLM Smart Fill           │
                      │   ├─ S-091: XML Conversion           │
                      │   ├─ S-092: Docker Export            │
                      │   ├─ S-093: Template Library         │
                      │   ├─ S-094: Multi-Tool Servers       │
                      │   ├─ S-095: OAuth 2.0                │
                      │   ├─ S-096: Team Workspaces          │
                      │   ├─ S-097: Webhook Notifications    │
                      │   ├─ S-098: Custom Domains           │
                      │   ├─ S-099: Server Versioning        │
                      │   ├─ S-100: Analytics Dashboard      │
                      │   └─ S-101: Credential Rotation      │
                      └──────────────────────────────────────┘
```

**Key Dependencies:**
1. Epic 00 must complete before any other epic can start
2. Epic 01 (URL parsing) and Epic 02 (auth) can proceed in parallel after Epic 00
3. Epic 03 (test calls) depends on Epic 01 + Epic 02 (needs URL and auth configured)
4. Epic 04 (field mapping) depends on Epic 03 (needs response data to map from)
5. Epic 05 (deployment) depends on Epic 01-04 (needs complete server configuration)
6. Epic 06 (advanced features) depends on Epics 01-05 (needs working product foundation)

**Critical Dependency Chains:**
- S-001-010 → S-016-040 → S-081-100 → S-101-140 → S-141-180 (sequential path)
- Parallelizable: (S-016-040) || (S-041-080) after S-001-010 completes
- Parallelizable: S-101-140 can run in parallel with S-081-100 once Epic 02 is done

---

## Implementation Phases

Recommended breakdown into 4 manageable phases with clear milestones:

### Phase 1: Foundation (Weeks 1-2, 35 story points)
**Epic 00 Complete**

**Goals:**
- Establish development infrastructure
- Deploy initial authentication and credential system
- Set up CI/CD pipeline and monitoring
- Establish data models and schema

**Stories:** S-001 through S-015

**Deliverables:**
- Kubernetes cluster running (single-node or multi-node dev)
- Woodpecker CI pipeline
- PostgreSQL database initialized with schema
- User authentication flow (login/signup)
- Encryption system for credentials
- Monitoring dashboard (Datadog/Prometheus)

**Acceptance:** Infrastructure ready to support feature development; team can deploy code changes within 5 minutes

---

### Phase 2: Core Product (Weeks 2-5, 95 story points)
**Epics 01, 02, 03 Complete**

**Goals:**
- Build server URL and parameter handling
- Implement authentication configuration flows
- Enable testing against live APIs
- Render responses in human-readable format

**Stories:** S-016 through S-100 (Epics 01, 02, 03)

**Deliverables:**
- Server creation wizard (URL input, parameter detection)
- Auth configuration UI (Bearer, API Key, Basic)
- Test execution engine
- Response document renderer (JSON and XML)
- Error handling and feedback UI

**Acceptance:** Users can create a server, configure auth, and test it against a real API. Response data displays in readable format.

**Key Milestones:**
- End of Week 2: URL parser and parameter detection shipped
- End of Week 3: Auth config UI complete for Bearer/API Key/Basic
- End of Week 4: Test call execution and basic JSON rendering
- End of Week 5: Full response rendering with XML support

---

### Phase 3: Ship It (Weeks 6-8, 75 story points)
**Epics 04 + 05 (Core)** — Deploy-ready MVP

**Goals:**
- Implement field mapping UI (click-to-select JSONPath)
- Generate MCP servers from user configuration
- Deploy servers to public endpoints
- Provide playground for testing
- Create server dashboard and management UI

**Stories:** S-101 through S-180 (Epics 04, 05, plus P0 items from Epics 01-03)

**Deliverables:**
- Field mapping visual interface
- Tool configuration and naming
- Server code generation (FastMCP)
- Live server deployment to platform
- Playground for testing deployed servers
- Server dashboard with CRUD operations
- Basic monitoring and error logs

**Acceptance:** MVP ready for public launch. Users can build, deploy, and test production MCP servers without code.

**Key Milestones:**
- End of Week 6: Field mapping UI and JSONPath extraction
- End of Week 7: Server generation and deployment
- End of Week 8: Dashboard, playground, and MVP polish

---

### Phase 4: Scale & Differentiate (Weeks 9-16, 80 story points)
**Epic 06 Complete** — Growth Stack

**Goals:**
- Add LLM-powered field mapping (smart fill)
- Support Docker export and self-hosted deployments
- Build template library for fast onboarding
- Enable team collaboration and multi-tool servers
- Implement enterprise features: OAuth, custom domains, versioning, analytics

**Stories:** S-090 through S-101 (Epic 06) + parallelizable improvements

**Deliverables:**
- LLM Smart Fill feature (Claude API integration)
- Docker export and Dockerfile generation
- Pre-built templates for Stripe, GitHub, Slack, etc.
- Multi-tool support in a single server
- OAuth 2.0 authentication flows
- Team workspaces with RBAC
- Webhook notifications for alerts
- Custom domain support with auto-TLS
- Server versioning and rollback
- Usage analytics dashboard
- Automated credential rotation

**Acceptance:** Full Growth Stack feature set. Product is enterprise-ready with collaboration, security, and operational features.

**Release Cadence:** 2-week sprints, releasing every other sprint (builds 1, 3, 5, 7)

---

## Story Count Summary

**Grand Total: 101+ User Stories**

Breakdown by priority:

| Priority | Count | Story Points | % of Total | Notes |
|----------|-------|--------------|-----------|-------|
| **P0 (Must ship MVP)** | 85 | ~190 | 74% | Epics 00-05, core features |
| **P1 (MVP+1 / Growth)** | 12 | ~80 | 26% | Epic 06, advanced features |
| **P2 (Post-Launch)** | 4-5 | ~10-15 | 2-3% | OAuth, advanced analytics |

**By Epic:**
- Epic 00: 15 stories (35 points) — P0 only
- Epic 01: 25 stories (45 points) — ~22 P0, ~3 P1
- Epic 02: 40 stories (50 points) — ~37 P0, ~3 P1
- Epic 03: 20 stories (35 points) — ~18 P0, ~2 P1
- Epic 04: 40 stories (40 points) — ~38 P0, ~2 P1
- Epic 05: 40 stories (45 points) — ~38 P0, ~2 P1
- Epic 06: 12 stories (80 points) — P1 only

---

## Critical Path

**Minimum viable set of stories for a functional MVP (launch-ready product):**

The critical path is the longest dependency chain from start to finish. These stories must be completed on schedule or the entire MVP timeline slips.

**Critical Path Stories (Sequential, 20 stories, ~125 points):**

1. **S-001-005:** Infra setup, Kubernetes, auth system (Week 1)
2. **S-016:** URL parser with parameter detection (Week 2)
3. **S-042:** Bearer token authentication (Week 3)
4. **S-081:** Test call execution (Week 4)
5. **S-085:** JSON response rendering (Week 4)
6. **S-101:** Field mapping UI (Week 5)
7. **S-102:** JSONPath selector (Week 5-6)
8. **S-110:** Tool definition (name, description) (Week 6)
9. **S-141:** Server code generation (FastMCP) (Week 7)
10. **S-150:** Deployment and launch (Week 7)
11. **S-030:** Dashboard with server cards (Week 7)
12. **S-160:** Playground for testing (Week 7-8)

**Parallelizable Paths (can run simultaneously with critical path):**
- Auth config variants (S-041, S-043, S-044) in parallel with URL parsing
- XML support (S-091) in parallel with JSON rendering
- Additional field mapping options in parallel with core JSONPath

**Critical Path Duration:** 8 weeks (40 calendar days) with a 2-person core team

**Slack in Schedule:** ~2-3 weeks built in for unforeseen issues, testing, and final polish

---

## Horizontal Scalability Roadmap

How Epic 05-06 stories map to user growth milestones:

### Milestone 1: 1,000 Active Users (Week 8-10)
**Focus:** Stability and basic observability

Stories to prioritize:
- S-160: Playground (basic testing)
- S-030-031: Dashboard and server management
- S-151: Error logging and debugging
- S-152: Rate limiting (basic 100 req/s per server)

**Infrastructure:** Single region (US-East), single database replica, basic monitoring

**Expected Load:** ~100 calls/second peak, ~1TB storage for call logs (90-day retention)

---

### Milestone 2: 10,000 Active Users (Week 12-14)
**Focus:** Performance and feature adoption

Stories to prioritize:
- S-093: Template library (drives adoption of popular APIs)
- S-100: Analytics dashboard (helps users understand usage patterns)
- S-097: Webhook notifications (ops efficiency)
- S-155: Caching for repeated calls (latency optimization)

**Infrastructure:** Two regions (US-East, EU-West), read replicas, Redis cache for call deduplication

**Expected Load:** ~1,000 calls/second peak, ~10TB storage

---

### Milestone 3: 100,000 Active Users (Week 16+, post-launch)
**Focus:** Enterprise features and security

Stories to prioritize:
- S-096: Team workspaces (agency and enterprise adoption)
- S-095: OAuth 2.0 (eliminates manual token management)
- S-098: Custom domain support (enterprise requirement)
- S-101: Credential rotation (security compliance)

**Infrastructure:** 3+ regions, advanced load balancing, sharding by workspace, dedicated credential store

**Expected Load:** ~10,000 calls/second peak, ~100TB storage

---

### Milestone 4: 1,000,000 Active Users (Future, Y2)
**Focus:** Global availability and high availability

Stories to prioritize (post-roadmap):
- Multi-region failover and disaster recovery
- Edge caching (Cloudflare or CDN)
- Advanced rate limiting and quota management
- Dedicated infrastructure tiers

**Infrastructure:** 6+ regions, edge caching, active-active deployment, sharding strategy per region

**Expected Load:** ~100,000 calls/second peak, ~1PB storage (tiered: hot/warm/cold)

---

## Team Allocation Suggestion

**Recommended team composition: 5-6 engineers, 8-week timeline**

### Team Structure

**Backend Team (2 engineers, full-time)**
- **Engineer 1: Core Server Logic**
  - Epics 01, 02, 03 (URL parsing, auth config, test calls)
  - Owns: parameter detection, auth validation, test execution engine
  - Skills: Rust, REST API design, error handling

- **Engineer 2: Field Mapping & Deployment**
  - Epics 04, 05 (field mapping, server generation, deployment)
  - Owns: JSONPath extraction, FastMCP integration, server code generation
  - Skills: Rust, JSON/XML parsing, code generation

**Frontend Team (2 engineers, full-time)**
- **Engineer 1: Server Creation Wizard**
  - Epics 01, 02, 03 UI (URL input, auth config, test results)
  - Owns: form design, real-time parameter detection, auth flow UX
  - Skills: React, TypeScript, form validation

- **Engineer 2: Dashboard & Field Mapping UI**
  - Epics 04, 05 UI (field mapper, tool config, dashboard, playground)
  - Owns: visual field selector, playground interface, server management
  - Skills: React, TypeScript, interactive UI, drag-and-drop

**DevOps/Infra (1 engineer, full-time)**
- **DevOps Engineer:**
  - Epic 00 (infrastructure setup, CI/CD, monitoring)
  - Epics 05 (deployment infrastructure)
  - Owns: Kubernetes, CI/CD pipeline, secrets management, monitoring
  - Skills: Kubernetes, Terraform, Woodpecker CI, Datadog

**Product/QA Support (1 engineer, 50% time)**
- **Product Engineer:**
  - Story acceptance criteria validation
  - Integration testing across epics
  - Production readiness checklist
  - Skills: Testing, product understanding, cross-functional coordination

### Sprint Allocation (2-week sprints)

**Sprint 1 (Weeks 1-2): Foundation**
- Backend 1 & 2: Full effort on Epic 00 infrastructure
- Frontend 1 & 2: Environment setup, component library, design system
- DevOps: Kubernetes, CI/CD, database setup
- Focus: Unblock all other teams

**Sprint 2 (Weeks 3-4): Core Features**
- Backend 1: S-016-020 (URL parsing, params, method detection)
- Backend 2: S-041-050 (Bearer, API Key, Basic auth)
- Frontend 1: S-016-020 UI (URL input, param display)
- Frontend 2: S-041-050 UI (auth selector, config forms)
- DevOps: Monitoring setup, logging infrastructure

**Sprint 3 (Weeks 5-6): Testing & Mapping**
- Backend 1: S-081-090 (test execution, error handling)
- Backend 2: S-101-115 (field mapping, JSONPath extraction)
- Frontend 1: S-081-090 UI (test button, response display)
- Frontend 2: S-101-115 UI (field mapper, click-to-select)
- DevOps: Call logging, metric collection

**Sprint 4 (Weeks 7-8): Deployment & MVP Polish**
- Backend 1: S-141-150 (server generation, deployment)
- Backend 2: S-151-165 (error tracking, basic analytics)
- Frontend 1: S-030-031 UI (dashboard, server list)
- Frontend 2: S-160-170 UI (playground, management UI)
- DevOps: Production readiness, load testing, security audit

**Sprints 5-8 (Weeks 9-16): Growth Stack (Parallel with post-MVP stabilization)**
- Rotate team members to Epic 06 features (LLM, Docker, templates, OAuth, analytics)
- One engineer stays on bug fixes and performance optimization for MVP
- Stagger rollout: release new features every 2 weeks (builds 1, 3, 5, 7)

### Parallelization Opportunities

**Weeks 3-4:** Backend 1 and 2 work on Epic 01 & 02 independently (no dependencies)
**Weeks 5-6:** All backends work on Epic 03 & 04 simultaneously (Epic 03 depends on 01+02, Epic 04 depends on 03)
**Weeks 7-8:** Backend 1 focuses on server generation (critical path), Backend 2 works on observability (can slip if needed)

### Contingency Planning

If any engineer leaves or gets sick:
- Critical path is S-016, S-042, S-081, S-101, S-141 (one backend engineer could handle, but would slip by 2 weeks)
- Frontend can cross-train on basic backend (critical for URL parsing and test execution)
- DevOps is the bottleneck: losing DevOps engineer = 1-week delay in CI/CD and deployment

---

## Risk and Mitigation

### Schedule Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| LLM integration (S-090) delays Growth Stack | Medium | 2 weeks | Build mock LLM responses; defer S-090 to post-launch |
| OAuth implementation is more complex than estimated | Medium | 1 week | Start OAuth early (Week 9); use oauth2 crate (proven) |
| Multi-region deployment adds complexity | Low | 2 weeks | Defer multi-region to Growth Stack; use single region for MVP |
| XML parsing has edge cases | Low | 3 days | Extensive test suite; use battle-tested quick-xml crate |

### Technical Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| Rust code generation is hard to debug | Medium | Performance | Use code generation libraries (quote, syn); test generated code extensively |
| FastMCP library has bugs or missing features | Low | 1 week | Review FastMCP source; contribute fixes upstream; fallback to manual MCP protocol |
| Encryption/decryption becomes bottleneck | Low | Performance | Benchmark AES-256-GCM; use hardware acceleration; cache decrypted credentials in sidecar |
| Document renderer doesn't handle large responses (>100MB) | Medium | 2 days | Implement streaming JSON parser; paginate large responses |

### Team Risks

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| Frontend and backend not in sync (API changes) | Medium | 3 days | Daily sync meetings; API contracts defined upfront; mock API for frontend |
| Rust expertise gaps (team less experienced) | Medium | 1 week | Code reviews with Rust expert; pair programming on complex logic |
| Product direction shifts mid-sprint | Low | 1+ week | Clear roadmap communicated; story freeze for 2 weeks; post-MVP backlog for new ideas |

---

## Effort Estimation Notes

**Story Point Scale (Fibonacci: 1, 2, 3, 5, 8, 13):**
- **1 point:** < 2 hours, trivial (UI label, config flag)
- **2 points:** 2-4 hours, simple (form validation, API endpoint)
- **3 points:** 4-8 hours, straightforward (feature with one integration)
- **5 points:** 8-16 hours, moderate (feature with multiple components, some unknowns)
- **8 points:** 16-32 hours, complex (multi-step feature, integration, testing)
- **13 points:** 32+ hours, very complex (architectural change, new system)

**Assumptions:**
- Team velocity: ~10-12 story points per engineer per week (conservative)
- 5 engineers × 10 points/week = 50 points/week for MVP
- MVP (190 points) / 50 points per week = ~4 weeks baseline, extended to 8 weeks for integration/testing/stabilization
- Growth Stack (80 points) = 1.5 weeks design + 6.5 weeks implementation

---

## Success Criteria & Metrics

### MVP Success (End of Week 8)

| Metric | Target | Validation |
|--------|--------|-----------|
| Server creation time | < 5 minutes (no code) | User study with 5 beta users |
| Feature completeness | All P0 stories shipped | Story acceptance checklist |
| User satisfaction (NPS) | > 30 (beta) | Post-beta survey |
| Production uptime | 99%+ | Monitoring dashboard |
| Performance (p50 latency) | < 200ms | Load testing |
| Deployment frequency | Daily pushes | CI/CD metrics |

### Growth Stack Success (End of Week 16)

| Metric | Target | Validation |
|--------|--------|-----------|
| Template adoption | 60% of new users choose template | Analytics |
| LLM smart fill usage | 40% of new servers | Analytics |
| Team workspace adoption | 20% of users create/join team | Analytics |
| User retention (30-day) | 50%+ | Cohort analysis |
| NPS improvement | 30 → 50+ | Survey |
| Enterprise pipeline | 3+ qualified leads | Sales CRM |

---

## Appendix: Quick Links to Epic Details

- [Epic 00: Foundation & Infrastructure](#epic-summary-table)
- [Epic 01: Server Creation](#epic-summary-table)
- [Epic 02: Authentication Configuration](#epic-summary-table)
- [Epic 03: Test Calls & Responses](#epic-summary-table)
- [Epic 04: Field Mapping](#epic-summary-table)
- [Epic 05: Deployment & Ops](#epic-summary-table)
- [Epic 06: Advanced Features](./EPIC_06_ADVANCED_FEATURES.md)

**Full Story Detail Files (by Epic):**
- [Epic 00 Stories](./EPIC_00_FOUNDATION.md) (future document)
- [Epic 01 Stories](./EPIC_01_SERVER_CREATION.md) (future document)
- [Epic 02 Stories](./EPIC_02_AUTH_CONFIG.md) (future document)
- [Epic 03 Stories](./EPIC_03_TEST_CALLS.md) (future document)
- [Epic 04 Stories](./EPIC_04_FIELD_MAPPING.md) (future document)
- [Epic 05 Stories](./EPIC_05_DEPLOYMENT.md) (future document)
- [Epic 06 Stories](./EPIC_06_ADVANCED_FEATURES.md) (this document)

---

## Document History

| Date | Version | Changes |
|------|---------|---------|
| 2026-03-28 | 1.0 | Initial version with 7 epics, 101 stories, 4 implementation phases, team allocation, and critical path |

