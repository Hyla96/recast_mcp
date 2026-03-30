# Epic 03: Builder UI — Core Creation Flow

**Product:** Dynamic MCP Server Builder
**Epic ID:** EPIC-03
**Date:** 2026-03-28
**Status:** Ready for Engineering
**Tech Stack:** React 19 + TypeScript + Vite + TailwindCSS + Tanstack Query + Zustand + React Router

**Scope:** Everything a user touches between landing on `/servers/new` and clicking "Deploy." This epic owns the full builder canvas: app scaffolding, authentication, the URL input stage, auth configuration, test execution, the document renderer, click-to-select field mapping, array normalization, tool naming, and the request body editor. Deployment confirmation lives in EPIC-04.

**Definition of Done (epic-level):** A user with a valid REST API endpoint and credentials can complete the full builder flow — paste URL, configure auth, run test, click fields, name tool — entirely without touching code or reading documentation, with all interactions completing within the performance budgets stated in each story.

---

## Story Index

| Story ID | Title | Points | Priority |
|----------|-------|--------|----------|
| S-040 | App shell and routing | 5 | P0 |
| S-041 | Clerk authentication integration | 5 | P0 |
| S-042 | Dashboard — server list | 5 | P0 |
| S-043 | URL input with parameter auto-detection | 8 | P0 |
| S-044 | Auth configuration panel | 5 | P0 |
| S-045 | Test call execution | 8 | P0 |
| S-046 | Sample JSON escape hatch | 3 | P0 |
| S-047 | Document renderer | 8 | P0 |
| S-048 | Click-to-select field mapping | 8 | P0 |
| S-049 | Array path normalization | 5 | P1 |
| S-050 | Tool naming and description form | 3 | P0 |
| S-051 | Request body builder (POST/PUT/PATCH) | 5 | P1 |

**Epic total:** 68 points

---

## Builder Navigation Model

This section defines cross-cutting UX and architecture decisions that apply to all builder stories (S-043 through S-051).

### Step chrome

The builder flow renders a **horizontal step indicator** at the top of the builder area showing all stages: `URL → Auth → Test → Field Mapping → Naming → Review`. The current stage is highlighted with `brand-500`. Completed stages show a checkmark icon. Future stages are muted (`on-surface-variant`). The step indicator is a shared `<StepIndicator>` component (not reimplemented per story). It is display-only in MVP — clicking a step does not navigate to it.

### Accordion layout model

The builder uses an **accordion/collapse-previous** layout. When the user clicks "Continue" to advance to the next step:
1. The completed step collapses to a **summary row** showing: step name, a green checkmark, and a one-line summary of the user's input (e.g., "GET https://api.example.com/customers/{id}" for the URL step, "Bearer Token" for the auth step).
2. The summary row has an "Edit" link that re-expands the step and collapses the current step.
3. The new step expands with a smooth transition (`transition-all duration-normal`).
4. The browser scrolls the new step into view (`scrollIntoView({ behavior: 'smooth', block: 'start' })`).

### Stage invalidation rules

Changing upstream data invalidates downstream stages. These rules are enforced by the `builderStore` state machine:

| Changed stage | Invalidated stages | Behavior |
|--------------|-------------------|----------|
| URL (method or URL) | Auth (only if host changes), Test, Field Mapping, Naming | Test response cleared. Selected fields cleared. Tool name re-derived. Auth preserved if same host. |
| Auth | Test, Field Mapping, Naming | Test response cleared. Selected fields cleared. Tool name preserved. |
| Test (new response) | Field Mapping, Naming | Selected fields cleared (response structure changed). Tool name preserved. |
| Field Mapping | Naming | Tool name preserved. No invalidation. |
| Naming | (none) | No downstream stages. |

When invalidation occurs, a toast notification appears: "Your [stage] changes require re-running later steps." The invalidated stages' summary rows show an amber warning badge "Needs update".

### Browser navigation

The builder lives at a single route (`/servers/new`). The browser back button exits the builder entirely. This is intentional for MVP. A `beforeunload` event listener warns the user if they have unsaved builder progress: "You have unsaved changes. Leave anyway?"

### Draft persistence

Builder state is persisted to `sessionStorage` (not `localStorage` — drafts are per-tab, not cross-tab) on every stage completion. On mount, the builder checks `sessionStorage` for a draft and offers to resume: "You have an unfinished server. Resume where you left off?" with "Resume" and "Start fresh" buttons. Drafts expire after 24 hours. Credential values are NEVER persisted to `sessionStorage` — only the auth type selection is preserved.

### Shared components

The following shared components are established in S-040 and used across all builder stories:

| Component | Location | Used by |
|-----------|----------|---------|
| `<StepIndicator>` | `src/components/builder/StepIndicator.tsx` | All builder steps |
| `<StepLayout>` | `src/components/builder/StepLayout.tsx` | All builder steps (consistent padding, heading, Continue/Back buttons) |
| `<PasswordInput>` | `src/components/ui/PasswordInput.tsx` | S-044 (all credential fields) |
| `<EncryptedFieldBadge>` | `src/components/ui/EncryptedFieldBadge.tsx` | S-044 (lock icon + tooltip) |
| `<SegmentedControl>` | `src/components/ui/SegmentedControl.tsx` | S-044, S-045, S-046 |
| `<JsonValidator>` | `src/lib/jsonValidator.ts` | S-046, S-051 (shared parse + line-number extraction) |
| `<Toast>` | `src/components/ui/Toast.tsx` | Global (stage invalidation, save confirmations) |

### Error boundary strategy

Every builder step is wrapped in a React Error Boundary (`<StepErrorBoundary>`). The fallback UI shows: "Something went wrong in this step" with a "Reset this step" button that clears the step's data in `builderStore` and re-renders. The Document Renderer (S-047) has its own dedicated error boundary with fallback: "Could not render this response. Try pasting a simpler JSON sample." This is critical because the renderer processes arbitrary `data: unknown` recursively.

### Scroll restoration

On step transitions, the browser scrolls to the top of the new step. React Router's built-in scroll restoration is configured in S-040 for route-level navigation. For in-page step transitions, `scrollIntoView` is called explicitly.

---

## S-040: App Shell and Routing

**Story ID:** S-040
**Title:** App shell and routing
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** None — this is the foundation for every other story in this epic.

### Description

As the engineering team, we need a fully scaffolded React 19 application with Vite, TailwindCSS, and React Router so that all subsequent stories have a stable routing and layout foundation to build on. This story delivers the skeleton only: routes, layout shell, theme system, and responsive breakpoints. No business logic.

### Acceptance Criteria

1. Running `npm run dev` starts the Vite dev server and renders the app at `localhost:5173` with no TypeScript errors in strict mode (`"strict": true`, `"noImplicitAny": true`, `"strictNullChecks": true`, `"noUncheckedIndexedAccess": true`, `"exactOptionalPropertyTypes": true`).
2. React Router v6+ is configured with the following routes, each rendering a named placeholder component with the route path visible on screen:
   - `/` — marketing or redirect to `/dashboard`
   - `/login` — unauthenticated entry point
   - `/dashboard` — server list (placeholder)
   - `/servers/new` — builder flow (placeholder)
   - `/servers/:id` — server detail (placeholder)
   - `/servers/:id/playground` — playground panel (placeholder)
3. A persistent app shell wraps all authenticated routes with: top navigation bar (logo left, user menu right), main content area, and optional sidebar slot. Unauthenticated routes (`/`, `/login`) render without the shell.
4. Dark mode and light mode are both implemented using Tailwind's `class` strategy (toggled via `<html class="dark">`). A toggle control in the nav bar switches between modes and persists the preference to `localStorage`. The selected class is applied before first paint to prevent flash.
5. The active theme class is readable by Cypress/Playwright selectors via `data-testid="theme-toggle"` and `data-theme="dark|light"` on the `<html>` element.
6. Layout is responsive across three breakpoints: mobile (`< 768px`), tablet (`768px–1279px`), desktop (`>= 1280px`). The nav bar does not overflow or clip on any breakpoint. Content area uses fluid widths with `max-w-7xl mx-auto px-4 sm:px-6 lg:px-8`.
7. TailwindCSS is configured with a custom design token layer: color palette (`brand-*`, `surface-*`, `text-*`, `border-*`), font sizes, spacing scale, and border radius values are defined in `tailwind.config.ts` and not duplicated as arbitrary values in component classes.
8. Path aliases are configured in both `vite.config.ts` and `tsconfig.json`: `@/` maps to `src/`, `@components/` maps to `src/components/`, `@stores/` maps to `src/stores/`, `@hooks/` maps to `src/hooks/`. All imports in the scaffolded files use aliases, not relative paths.
9. A 404 catch-all route renders a "Page not found" screen with a link back to `/dashboard`. It does not render the authenticated shell.
10. `npm run build` produces a Vite production bundle with no warnings. Bundle is tree-shaken. React Router and TailwindCSS are the only non-dev dependencies added in this story.
11. **Vitest + React Testing Library** are configured as dev dependencies. `vitest.config.ts` extends `vite.config.ts`. A sample test file `src/lib/__tests__/sample.test.ts` passes with `npm run test`. The `test` script is added to `package.json`. `@testing-library/react`, `@testing-library/jest-dom`, and `@testing-library/user-event` are installed.
12. **`@types/react` and `@types/react-dom`** are upgraded to `^19.x` for React 19 type compatibility. The `forwardRef` wrapper is not used anywhere — React 19 passes `ref` as a regular prop.
13. **TypeScript target** is updated to `ES2022` (aligned with technical notes). `tsconfig.json` includes `"noImplicitAny": true`, `"strictNullChecks": true`, `"noUncheckedIndexedAccess": true`, `"exactOptionalPropertyTypes": true`.
14. A global **React Error Boundary** component (`<AppErrorBoundary>`) is implemented wrapping the router outlet. A specialized `<StepErrorBoundary>` variant is created for builder steps with step-specific reset logic.
15. A **toast notification system** is implemented as a React context + portal. Toasts render at bottom-right, support success/error/info variants, auto-dismiss after 5 seconds (errors persist), and use `aria-live="polite"` (success/info) or `aria-live="assertive"` (errors). Maximum 3 visible toasts stacked.
16. The builder **shared components** are scaffolded as empty shells: `<StepIndicator>`, `<StepLayout>`, `<PasswordInput>`, `<EncryptedFieldBadge>`, `<SegmentedControl>`, `<Toast>`. Their interfaces are typed but implementations are placeholder.
17. A `src/styles/tokens.css` file defines all design system tokens as CSS custom properties on `:root` (light) and `.dark` (dark), including the full `brand-*` numeric scale. Tailwind references these variables via `tailwind.config.ts`.
18. A **`vite-env.d.ts`** file declares custom environment variable types: `VITE_CLERK_PUBLISHABLE_KEY`, `VITE_API_BASE_URL`, `VITE_WS_BASE_URL`.
19. **Scroll restoration** is configured on `createBrowserRouter` for route-level navigation.

### Technical Notes

- Use `createBrowserRouter` with `RouterProvider` (not `BrowserRouter`) for future data router compatibility.
- Implement layout routes via React Router's nested route + `<Outlet />` pattern. The authenticated shell is a layout route; `/login` and `/` are siblings outside it.
- Dark mode toggle: read `localStorage` in an inline `<script>` in `index.html` before React hydrates, to avoid FOUC. Store the value under key `"mcp-theme"`. The inline `<script>` tag must be a plain `<script>` tag (NO `type="module"`, NO `defer`, NO `async`). It reads `localStorage.getItem('mcp-theme')`, falls back to `window.matchMedia('(prefers-color-scheme: dark)').matches`, and sets `document.documentElement.classList.add('dark')` before React mounts. Example:
  ```html
  <script>
    (function(){var t=localStorage.getItem('mcp-theme');if(t==='dark'||(t==null&&window.matchMedia('(prefers-color-scheme:dark)').matches)){document.documentElement.classList.add('dark');document.documentElement.setAttribute('data-theme','dark')}else{document.documentElement.setAttribute('data-theme','light')}})();
  </script>
  ```
- Do not use `@apply` in CSS files except for base resets. Keep utility classes in JSX.
- TypeScript target: `"ES2022"`. Module: `"ESNext"`. `moduleResolution: "Bundler"`.
- Zustand store slice for UI state (`theme`, `sidebarOpen`) is scaffolded in this story even though nothing consumes it yet.
- Zustand store uses `immer` middleware from the start. Install `immer` as a dependency. All builder store slices use `produce` for state updates, which avoids painful spread chains with `noUncheckedIndexedAccess` enabled.
- The Zustand `builderStore` is defined with explicit slice boundaries: `urlSlice`, `authSlice`, `testSlice`, `mappingSlice`, `namingSlice`, and a top-level `currentStage` + `stageValidation` map. Each slice is independently resettable.
- Mobile navigation: the nav bar collapses to a hamburger menu on mobile (`< 768px`). The dark mode toggle is accessible inside the mobile menu drawer AND as a standalone toggle in the hamburger menu header.

---

## S-041: Clerk Authentication Integration

**Story ID:** S-041
**Title:** Clerk authentication integration
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-040 (app shell and routing must exist before routes can be protected)

### Description

As a user, I want to sign up and sign in using email/password or OAuth providers so that my MCP servers are private to my account and accessible across sessions.

As the engineering team, we need Clerk integrated as the identity provider so that authenticated state is available to all components and API calls include a valid session token for backend verification.

### Acceptance Criteria

1. `@clerk/clerk-react` is installed and `<ClerkProvider>` wraps the entire app in `main.tsx`. The publishable key is read from `import.meta.env.VITE_CLERK_PUBLISHABLE_KEY` and never hardcoded.
2. The `/login` route renders Clerk's `<SignIn>` component. Successful sign-in redirects to `/dashboard`. If the user is already authenticated, visiting `/login` redirects immediately to `/dashboard`.
3. The `/login` route also exposes a "Create account" path that renders Clerk's `<SignUp>` component inline or via tab toggle. Successful sign-up redirects to `/dashboard`.
4. All routes except `/` and `/login` are protected. Unauthenticated requests to any protected route redirect to `/login` with a `redirect_url` query param so the user is returned to the originally requested URL after sign-in.
5. The authenticated shell nav bar renders a user profile dropdown. The dropdown contains: user's avatar (from Clerk), display name, email address (read-only), and a "Sign out" button. Clicking "Sign out" calls `signOut()` and redirects to `/login`.
6. `useAuth().getToken()` is available app-wide. A shared `fetchWithAuth` utility wraps `fetch` and calls `getToken()` before each request to attach `Authorization: Bearer <token>`. Clerk's SDK caches tokens internally — do not add a module-level token cache. Individual `useQuery`/`useMutation` hooks use `fetchWithAuth` in their `queryFn`/`mutationFn`. A `queryClient` is configured with sensible defaults: `staleTime: 30_000` (30s), `retry: 1` (fail fast), `gcTime: 300_000` (5 min). Individual queries override these as needed.
7. Clerk's `<UserButton>` or equivalent is used for the profile dropdown, styled to match the app's design tokens. The avatar falls back to the user's initials if no profile image is set.
8. TypeScript: `useUser()` and `useAuth()` return values are consumed with proper null guards. No `!` non-null assertions on Clerk hooks.
9. A Clerk `<RedirectToSignIn>` guard component wraps the authenticated layout route in the router config. It is not implemented as an ad-hoc check in individual page components.
10. Sign-in and sign-up flows work in both dark and light mode without Clerk's default styles overriding the app's theme. Clerk's `appearance` prop is used to pass `baseTheme` and `variables` matching the app's color tokens.

### Technical Notes

- Use Clerk's React SDK v5+ (compatible with React 19's concurrent mode).
- Do not use Clerk's `withAuth` HOC (deprecated). Use hooks exclusively.
- `fetchWithAuth` calls `getToken()` on every request. Clerk's React SDK caches the token internally and only refreshes when needed — there is no performance penalty. Do not cache the token in a module-level variable (causes stale-token bugs on session refresh). Handle 401 responses globally: if any `fetchWithAuth` call receives a 401, call `signOut()` and redirect to `/login`. This covers the case where Clerk's session expires between token cache refreshes.
- Session expiry: Clerk handles token refresh automatically. The app does not need to implement refresh logic, but should handle 401 responses from the backend by calling `signOut()` and redirecting.
- Environment variables required: `VITE_CLERK_PUBLISHABLE_KEY`, `VITE_API_BASE_URL`, `VITE_WS_BASE_URL`. All documented in `.env.example`.

---

## S-042: Dashboard — Server List

**Story ID:** S-042
**Title:** Dashboard — server list
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-040 (routing), S-041 (auth — list is user-scoped)

### Description

As a user, I want to see all my MCP servers at a glance with their current status so that I can quickly identify which servers are healthy, which need attention, and navigate to any server's detail page or playground.

### Acceptance Criteria

1. The `/dashboard` route fetches the authenticated user's server list from `GET /api/v1/servers` via a Tanstack Query `useQuery`. The query key is `['servers', userId]`.
2. Each server is rendered as a card. The card displays:
   - Server name (primary text, bold)
   - Status badge: "Active" (green), "Error" (red), "Inactive" (gray). The badge text and color are driven by the server's `status` field in the API response.
   - Endpoint URL (truncated to fit one line with `text-ellipsis`, full URL on hover via `title` attribute)
   - "Last call" timestamp: formatted as relative time ("3 minutes ago", "2 hours ago", "Yesterday") using `Intl.RelativeTimeFormat`. If no calls have been made, show "Never".
   - Call count (24h): displayed as "1,234 calls today" using `Intl.NumberFormat`. If zero, show "No calls today".
3. Clicking anywhere on a card navigates to `/servers/:id`. The card is a single focusable element (not nested interactive elements) with `role="link"` or wrapped in a `<Link>`. Tab navigation reaches each card and activates on Enter.
4. A "New server" button is always visible in the top-right of the dashboard. Clicking navigates to `/servers/new`. The button has `data-testid="new-server-btn"`.
5. Empty state: when the server list is empty (zero items), the main content area shows an illustration placeholder, heading "No servers yet", subtext "Expose your first REST API to Claude in 90 seconds", and a "Create your first server" CTA button linking to `/servers/new`. The CTA has `data-testid="create-first-server-cta"`.
6. Loading state: while the query is in `pending` state, render skeleton cards (three skeleton items by default) matching the card layout. Skeletons use a CSS pulse animation via `animate-pulse`. No spinner.
7. Error state: if the query fails, render an error card with message "Failed to load servers" and a "Try again" button that calls `refetch()`. The error state replaces the card grid, not the page header.
8. Live status updates: the query polls every 30 seconds (`refetchInterval: 30_000`). Additionally, if a WebSocket connection to `wss://api.example.com/ws/servers` is available, incoming `server.status_changed` events trigger an immediate `queryClient.invalidateQueries(['servers'])` without waiting for the next poll interval. WebSocket connection failure falls back gracefully to polling only (no error shown to user).
9. The server grid is responsive: three columns on desktop (`>= 1280px`), two columns on tablet (`768px–1279px`), one column on mobile (`< 768px`).
10. Servers are sorted by `updatedAt` descending (most recently modified first). The sort is applied client-side after fetch; the API is not expected to sort. A future sort control is not in scope for this story.

### Technical Notes

- The WebSocket connection is established once on dashboard mount and torn down on unmount. Use a custom `useServerStatusSocket` hook that internally uses `useEffect` and returns a `connectionState: 'connected' | 'disconnected' | 'error'`.
- Server list state lives entirely in Tanstack Query cache. Do not duplicate it in Zustand.
- Relative time formatting: compute the diff from `Date.now()`. Use thresholds: `< 60s` → "just now", `< 60m` → "X minutes ago", `< 24h` → "X hours ago", `< 48h` → "Yesterday", else → absolute date `MMM D, YYYY`.
- Card hover state: `ring-2 ring-brand-500` on hover/focus for clear interactive affordance.
- WebSocket message schema for `server.status_changed`: `{ type: "server.status_changed", payload: { serverId: string, status: "active" | "error" | "inactive", timestamp: string } }`. The hook validates the message shape before calling `invalidateQueries`.
- WebSocket URL is derived from `VITE_WS_BASE_URL` environment variable: `${VITE_WS_BASE_URL}/ws/servers`.
- Debounce WebSocket-triggered invalidations: if multiple `server.status_changed` events arrive within 500ms, coalesce them into a single `invalidateQueries` call. This prevents re-render storms for users with many active servers.
- Use `useEffectEvent` (React 19) for the WebSocket message handler inside the `useServerStatusSocket` hook. This avoids putting `queryClient` in the `useEffect` dependency array (which would cause reconnections on every render).
- For the server list at scale (100+ servers), note that `@tanstack/virtual` should be integrated as a follow-up if performance degrades. Not in scope for this story.

---

## S-043: URL Input with Parameter Auto-Detection

**Story ID:** S-043
**Title:** URL input with parameter auto-detection
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-040 (routing and shell), S-041 (auth for saving drafts)

### Description

As a user, I want to paste a REST API URL into the builder and immediately see all detected path and query parameters listed below, so I know the system has correctly understood the API's interface before I proceed to authentication and field mapping.

This is the first stage of the builder flow (`/servers/new`, step 1 of N). It must work correctly for the full range of real-world URL patterns encountered in production APIs.

### Acceptance Criteria

1. The builder page (`/servers/new`) renders a URL input field as the primary element. The field has `placeholder="https://api.example.com/customers/{customer_id}"`, `data-testid="url-input"`, and `type="url"` (but validation is custom, not browser-native).
2. A method selector dropdown sits to the left of the URL input. Options: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`. Default selection: `GET`. The dropdown has `data-testid="method-select"`. Method and URL together form the "endpoint" concept throughout the app.
3. URL parsing runs on every `onChange` event (debounced 150ms) and produces a `ParsedUrl` structure:
   ```typescript
   interface ParsedUrl {
     protocol: 'http' | 'https' | null;
     host: string | null;
     pathname: string | null;
     pathParams: DetectedParam[];
     queryParams: DetectedParam[];
     isValid: boolean;
     error: UrlParseError | null;
   }

   interface DetectedParam {
     name: string;
     kind: 'path' | 'query';
     inferredType: 'string' | 'number' | 'boolean';
     rawValue: string | null; // populated for query params with values
   }

   type UrlParseError =
     | { code: 'MISSING_PROTOCOL'; message: string }
     | { code: 'INVALID_HOST'; message: string }
     | { code: 'MALFORMED_URL'; message: string };
   ```
4. Path parameters are detected by the pattern `{param_name}` (curly brace syntax). The regex handles: simple names (`{id}`), underscored names (`{customer_id}`), camelCase (`{orderId}`), and hyphenated names (`{order-id}`). Each detected path parameter appears as a row below the URL field with: a tag reading "Path param", the parameter name, a type selector dropdown (`string | number | boolean` — default `string`), and a text input for an example value (optional, placeholder "Example value").
5. Query parameters are detected from the URL's query string (anything after `?`). Each `key=value` pair produces one row with: a tag reading "Query param", the key as the parameter name (editable), inferred type (`string` by default; `number` if the value parses as `parseFloat`; `boolean` if the value is `"true"` or `"false"`), the raw value pre-filled as the example value.
6. Parameter names are editable in-place. Editing a parameter name updates the name on `blur`. Renaming produces no side effects in this story (field mapping in S-048 will reference the name at mapping time).
7. The parameter detection result updates in real time as the user types. If the user types a URL that previously had `{id}` and then removes the parameter, the row for `id` disappears immediately after the debounce.
8. Detected parameters are rendered in a section below the URL/method row labeled "Detected parameters". If no parameters are detected and the URL is valid, show "No parameters detected" in muted text. If the URL field is empty, the section is not rendered at all.
9. Validation: if the URL is invalid, an inline error message appears below the URL input field (not a toast). The error message is specific: "Missing protocol — URL must start with https://" or "Invalid host — check the domain name" or "Malformed URL — could not parse the URL structure." The URL input field gets a red ring (`ring-2 ring-red-500`). No error is shown while the field is empty.
10. The current step's state (URL, method, detected params, user edits to param names and types) is stored in a Zustand `builderStore` slice so that navigating away and returning does not lose progress within the same browser session.
11. The "Continue" button (advancing to auth configuration, step 2) is disabled until the URL is valid (regardless of whether parameters are detected). It has `data-testid="url-step-continue"`.
12. URL parsing logic is extracted into a pure function `parseRestUrl(url: string): ParsedUrl` in `src/lib/urlParser.ts`. This function has unit tests covering: zero params, one path param, multiple path and query params, query params with no values, URL with no path, URL with special characters in path, invalid URLs (each error code).

### Technical Notes

- Use the browser's `URL` constructor for initial parsing — it handles most edge cases and throws on truly malformed inputs. Layer the path/query param detection on top of the parsed result.
- The path param regex: `/\{([a-zA-Z_][a-zA-Z0-9_-]*)\}/g`. Do not attempt to detect OpenAPI-style `:param` syntax in this story (potential future story).
- Type inference for query params: attempt `Number(value)` first, then check `"true"/"false"`, then fall back to `string`. Never infer `null` or `undefined`.
- Debounce: use a `setTimeout`-based debounce hook (`useDebounce`) for the URL parsing. `useDeferredValue` is not appropriate here — it defers rendering, not computation. The 150ms debounce prevents excessive re-parsing during rapid typing.
- Zustand builder store: model the builder flow as a state machine with stages `['url', 'auth', 'test', 'mapping', 'naming', 'review']`. The current stage and each stage's data are stored separately so partial resets are possible.

---

## S-044: Auth Configuration Panel

**Story ID:** S-044
**Title:** Auth configuration panel
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-043 (URL input must be complete; auth is step 2 of the builder flow)

### Description

As a user, I want to configure authentication for my API by selecting an auth type and entering my credentials in a type-appropriate form, so that the platform can proxy calls to my private API on my behalf without me having to re-enter credentials on every test.

### Acceptance Criteria

1. The auth configuration panel is step 2 of the builder flow. It renders below the URL/params section (or as the next visible step in a stepped layout) after the user clicks the "Continue" button from S-043. The panel has `data-testid="auth-panel"`.
2. An auth type selector renders four options as a segmented control: `None`, `Bearer Token`, `API Key`, `Basic Auth`. Default selection is `None`. The `<SegmentedControl>` component (from S-040 shared components) implements `role="radiogroup"` with each option as `role="radio"`. Arrow keys navigate between options (single tab stop for the group). Changing the selection shows the corresponding credential inputs and hides the others with no page jump (content appears in place, smoothly via `transition-all duration-normal`). The component has `data-testid="auth-type-selector"`.
3. **Bearer Token configuration:**
   - A single input labeled "Bearer token" with `type="password"` (characters masked by default).
   - A "Show / Hide" toggle button using the shared `<PasswordInput>` component. The toggle is `type="button"` (does not submit forms), with `aria-label="Show bearer token"` / `aria-label="Hide bearer token"`, keyboard accessible (Enter/Space activates).
   - A lock icon using the shared `<EncryptedFieldBadge>` component with tooltip reading "Encrypted at rest using AES-256. Never logged or displayed again after save." The tooltip is visible on hover and on focus (keyboard accessible).
   - Validation on blur: if the field is non-empty and fewer than 10 characters, show inline error "Token appears too short — check you copied the full value."
4. **API Key configuration:**
   - A toggle to choose placement: "Header" or "Query parameter" (default: "Header"). `data-testid="apikey-placement-toggle"`.
   - A "Key name" input with an autocomplete datalist containing common values: `X-API-Key`, `Authorization`, `X-Auth-Token`, `Api-Key`. The input accepts any value; the datalist provides suggestions only.
   - A "Key value" input with `type="password"`, Show/Hide toggle, and the same lock icon + tooltip as Bearer.
   - Validation on blur: key name must be non-empty and match `/^[a-zA-Z0-9_-]+$/`; show inline error if not.
5. **Basic Auth configuration:**
   - A "Username" input with `type="text"`, `autocomplete="off"`.
   - A "Password" input with `type="password"`, Show/Hide toggle, and the same lock icon + tooltip.
   - Validation on blur: both fields must be non-empty; show inline error per field.
6. All credential inputs are `autocomplete="off"` and `autocomplete="new-password"` as appropriate to prevent browser autofill from populating API credentials with personal account credentials.
7. The selected auth type and credential values (except masked display) are stored in the `builderStore` auth slice. Credential values in the store are stored as-entered (plain text in memory); they are never written to `localStorage` or `sessionStorage`. They are transmitted to the backend only over HTTPS as part of the test call (S-045) or the deploy action (EPIC-04).
8. A "Back" link returns to the URL step without losing URL/params data. The "Continue" button advances to test call execution (S-045). The Continue button is enabled when auth type is `None` OR when the selected auth type has all required fields filled and passing validation.
9. When auth type is `None`, a helper text with `role="status"` and `aria-live="polite"` reads: "No authentication. Proceed only if the endpoint is publicly accessible." in muted warning amber text. Screen readers announce this when the selection changes to `None`.
10. Each credential input's Show/Hide toggle is keyboard accessible (Tab reaches it, Enter or Space activates it). The button must not submit the form.

### Technical Notes

- The segmented control component should be a reusable `<SegmentedControl options={...} value={...} onChange={...} />` component, not inline markup in the auth panel. It will be reused in S-045 and S-046.
- Do not store the auth type in the URL (query params). Auth configuration is ephemeral builder state in Zustand.
- The Show/Hide toggle pattern: manage `isVisible` state per field in local component state. Do not lift it to Zustand.
- Credential transmission: the backend never receives credentials in GET query parameters. Always POST to the proxy endpoint with credentials in the request body (encrypted by TLS).

---

## S-045: Test Call Execution

**Story ID:** S-045
**Title:** Test call execution
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-043 (URL and params), S-044 (auth config — together they form the complete request specification)

### Description

As a user, I want to click "Test" and see the actual response from my API rendered in the app, so that I can confirm the platform can reach my API before I spend time configuring field mapping.

This is the critical trust moment in the builder flow. The test must be transparent (user sees real response data), fast (feedback within the request's natural latency), and honest about failure modes (differentiated error messages for different failure types).

### Acceptance Criteria

1. The test execution section is step 3 of the builder flow. It renders after auth config. It contains: parameter value inputs (one per detected param from S-043), a "Test" button, and a results area.
2. For each detected parameter (path and query), an input field is rendered with label equal to the parameter name, placeholder "Enter example value", and the example value from S-043 pre-filled if provided. Parameter inputs are grouped: "Path parameters" and "Query parameters" in separate labeled fieldsets. If no parameters were detected, the fieldsets are not rendered.
3. The "Test" button has `data-testid="test-call-btn"`. Clicking it:
   - Disables the button and replaces its label with a spinner and "Testing..." text.
   - Shows a "Cancel" link adjacent to the button (`data-testid="test-call-cancel"`).
   - Sends `POST /api/v1/proxy/test` to the platform backend with the payload: `{ url, method, pathParams, queryParams, auth, body? }`. The backend executes the actual HTTP call to the upstream API and returns the proxied response.
4. **Success response (2xx from upstream):** the results area renders the parsed JSON response via the Document Renderer (S-047). The test section header updates to show a green checkmark and "200 OK" (or the actual status code). The response is stored in `builderStore.testResponse`. After a successful test, focus is programmatically moved to the Document Renderer heading (`<h3>` with `tabIndex={-1}`) so keyboard users can immediately navigate the rendered output.
5. **4xx response from upstream:** display a red banner with "API returned [status code]: [status text]". Below the banner, render the response body as-is (could be JSON or plain text, render accordingly). Do not proceed the user automatically; show a "Try different values" suggestion.
6. **5xx response from upstream:** display a red banner "The upstream API returned a server error ([status code]). This may be temporary — try again in a moment." with a "Retry" button.
7. **Timeout (no response within 30 seconds):** display "The request timed out after 30 seconds. The API may be unreachable or taking too long." Show "Try again" and "Use sample response instead" (linking to S-046 behavior).
8. **Connectivity error (platform could not reach the host):** display "Could not connect to [host]. The API may require a VPN, be behind a firewall, or the URL may be incorrect." Show "Use sample response instead" link prominently (`data-testid="use-sample-response-link"`).
9. **Cancellation:** clicking "Cancel" aborts the in-flight request client-side. The Cancel link is only visible while a test is in progress.
10. After a successful test, a "Continue to field mapping" button appears (`data-testid="proceed-to-mapping-btn"`). After a failed test, the "Continue" button is replaced with "Skip test and use sample response" (which activates S-046) and "Fix and retry". The user cannot proceed to field mapping without either a successful test or a valid sample JSON (from S-046).
11. The test call is managed via a Tanstack Query `useMutation`. Loading, success, and error states are driven by the mutation state, not local `useState` booleans. Retry logic: no automatic retries (the user decides when to retry).
12. The 30-second timeout is enforced client-side via `AbortController`. If the fetch is aborted by timeout, the error falls into the "timeout" branch (AC 7), not the generic error branch.

### Technical Notes

- The platform's proxy endpoint (`/api/v1/proxy/test`) is responsible for the actual HTTP call. The frontend never calls the upstream API directly (CORS, credential security). The frontend sends credentials to the platform backend over HTTPS; the backend makes the upstream call server-side.
- Request ID and cancellation: The proxy endpoint (`POST /api/v1/proxy/test`) is **synchronous for MVP** — it blocks until the upstream response arrives. This means the `DELETE /api/v1/proxy/test/:requestId` cancel endpoint is NOT available in MVP. Client-side cancellation via `AbortController` aborts the `fetch` call to the platform backend, which in turn should abort the upstream request (platform backend must propagate the abort signal). Server-side cancel with request ID is a post-MVP enhancement.
- The `useMutation` `mutationFn` returns a discriminated union result type:
  ```typescript
  type TestCallResult =
    | { outcome: 'success'; status: number; body: unknown }
    | { outcome: 'client_error'; status: number; body: unknown }
    | { outcome: 'server_error'; status: number; body: unknown }
    | { outcome: 'timeout' }
    | { outcome: 'connectivity_error'; host: string };
  ```
  The `onSuccess` callback of `useMutation` receives this union and branches on `outcome`. Do not use `onError` for non-network errors (4xx, 5xx, timeouts that resolved cleanly from the proxy).
- The `mutationFn` must catch `AbortError` from the `AbortController` timeout and return `{ outcome: 'timeout' }` rather than throwing. This keeps all outcomes in the `onSuccess` callback. Only actual network failures (fetch throws for reasons other than abort) should fall through to `onError`.
- Platform API errors (platform itself is down, returns 500): handle in `onError`. Show: "The Recast platform is temporarily unavailable. This is not an issue with your API." with a "Try again" button.

---

## S-046: Sample JSON Escape Hatch

**Story ID:** S-046
**Title:** Sample JSON escape hatch
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-045 (escape hatch is triggered from test call failure states), S-047 (valid JSON feeds the document renderer)

### Description

As a user with an API behind a firewall, VPN, or otherwise unreachable from the platform's servers, I want to paste a sample JSON response manually so that I can still build an MCP server without needing the platform to make a live test call.

### Acceptance Criteria

1. The "Use sample response" UI is activated by: (a) clicking the "Use sample response" link shown after a connectivity error (S-045 AC 8), or (b) clicking "Skip test and use sample response" after any failed test (S-045 AC 10). The user can also activate it proactively before attempting a test via a secondary "I'll paste a sample response" link below the Test button (lower visual prominence than the Test button). This link has `data-testid="sample-response-trigger"`.
2. Activation replaces the test call results area with a `<textarea>` labeled "Paste your API's JSON response here". The textarea has `data-testid="sample-json-input"`, `rows={12}`, `spellCheck={false}`, `autoCapitalize="none"`, and `fontFamily: monospace` in its inline style.
3. JSON validation runs on every `onChange` event (debounced 300ms). If the content is valid JSON, no error is shown. If the content is not valid JSON, an inline error message appears below the textarea. The error message includes the specific parse failure location: "Invalid JSON on line 4: Unexpected token '}'" by extracting position information from the `SyntaxError` thrown by `JSON.parse`. Line number is computed by counting newlines up to the error's position in the string.
4. When valid JSON is entered, it is immediately passed to the Document Renderer (S-047) and rendered in the results area. The textarea remains visible above the rendered output so the user can continue editing.
5. A server configured with a sample response (rather than a live test) is marked as "unverified" in the `builderStore` via a boolean flag `isUnverified: true`. This flag is persisted through the deploy flow and stored in the server record.
6. In the dashboard (S-042) and server detail page (S-063 in EPIC-04), unverified servers display a yellow badge "Sample response — not live-tested" with a tooltip "This server was built using a pasted sample response. Live behavior may differ. Edit the server to run a real test call."
7. The textarea accepts responses up to 500KB without UI degradation. Input larger than 500KB is rejected with an inline error "Response too large — maximum 500 KB." (No need to test exactly at the boundary in unit tests; test at 1KB, 100KB, and 501KB.)
8. A "Clear and switch back to live test" link (`data-testid="back-to-live-test"`) allows the user to dismiss the textarea and return to the test call UI without losing the URL/auth configuration.

### Technical Notes

- JSON parse error line number extraction: `JSON.parse` throws a `SyntaxError` with a message like `"Unexpected token } in JSON at position 47"`. Extract the position integer, then count `\n` characters in `text.slice(0, position)` to get the line number. This is not always precise for all JSON.parse implementations — document the approximation in a code comment.
- JSON validation and line-number extraction use the shared `parseJsonWithLineNumbers(input: string): { ok: true; value: unknown } | { ok: false; error: string; line: number }` utility from `src/lib/jsonValidator.ts`. This function is shared with S-051. Do not implement the parsing logic inline.
- For very large pastes (approaching 500KB), schedule validation via `requestIdleCallback` instead of a simple `setTimeout` debounce. This prevents main-thread blocking on large inputs.
- The type stored in Zustand for `testResponse` is `unknown` (same as `DocumentRenderer`'s `data` prop). Both the live test path and the sample JSON path write to the same store field.
- The `<textarea>` should not use a controlled React input for large pastes (performance). Use an `uncontrolled` pattern with a `ref` and `onChange` for debounced validation only. Store the validated parsed value (not the raw string) in Zustand.
- Do not attempt to syntax-highlight the textarea. That is a post-MVP enhancement.

---

## S-047: Document Renderer

**Story ID:** S-047
**Title:** Document renderer
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-045 or S-046 must provide a parsed JSON object to render. S-048 depends on this component for click-to-select.

### Description

As a user, I want the API response rendered as a human-readable, structured document — not raw JSON — so that I can understand what data the API returns and confidently select the fields I want to expose to Claude.

The Document Renderer is the centerpiece of the builder UX. It must make arbitrary JSON legible to non-technical users while preserving enough structural information for engineers to understand the data shape.

### Acceptance Criteria

1. The Document Renderer accepts a single prop: `data: unknown` (the parsed JSON from a test call or sample paste). It renders a structured, formatted view of that data. It does not render raw JSON strings. It has `data-testid="document-renderer"`.
2. **Field name formatting:** all object keys are displayed as human-readable labels. Transformation rules applied in order:
   - Strip leading underscores (`_id` → `Id`)
   - Split `snake_case` on underscores (`customer_name` → `Customer Name`)
   - Split `camelCase` on uppercase letters (`orderId` → `Order Id`)
   - Split `PascalCase` on uppercase letters (`OrderId` → `Order Id`)
   - Title-case the result
   The original key is stored in a `data-field-key` attribute on the rendered label element for use by S-048.
3. **Scalar value rendering:**
   - `string`: rendered as-is, in regular weight text.
   - `number`: formatted with `Intl.NumberFormat`. If the key name contains `price`, `amount`, `cost`, `total`, or `fee` (case-insensitive), format as USD currency (`$1,234.56`). If the key name contains `percent` or `rate` and the value is between 0 and 1, multiply by 100 and append `%`. Otherwise, format as a plain number with thousands separators.
   - `boolean`: rendered as a colored pill — "Yes" (green) for `true`, "No" (red) for `false`.
   - `null`: rendered as "—" (em dash) in muted gray text with `aria-label="empty"`.
   - ISO 8601 date strings (matching `/^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}.*)?$/`): formatted using `Intl.DateTimeFormat` in the user's locale, showing date and time if the string includes a time component.
4. **Object rendering:** nested objects are rendered as collapsible sections. The section header shows the formatted key name with a chevron icon. Sections are expanded by default for the first two levels of nesting; deeper levels are collapsed by default. Toggle with click or Enter/Space.
5. **Array rendering:** arrays are inspected to determine their content type:
   - Array of objects: rendered as a table. Column headers are derived from the keys of the first element, formatted per AC 2. Up to 5 rows are shown; a "Show N more" button (expandable) handles additional rows.
   - Array of scalars: rendered as an inline comma-separated list or a `<ul>` if more than 5 items.
   - Empty array: rendered as "No items" in muted text.
6. Every rendered value (scalars, array cells, expanded object fields) is wrapped in a clickable element that emits an `onFieldSelect` callback. The wrapper has `data-testid="renderer-value"` and `data-jsonpath="<the JSONPath for this value>"`. Clickable values show a `ring-2 ring-brand-300` highlight on hover to communicate selectability.
7. Selected values (managed by S-048's state) are highlighted with `bg-brand-100 dark:bg-brand-900 ring-2 ring-brand-500`. The renderer receives a `selectedPaths: Set<string>` prop and applies this highlight without internal state.
8. The renderer handles pathological inputs gracefully: `null` top-level data renders "No data", `[]` top-level renders "Empty response", non-object/non-array top-level scalars render the formatted value with label "Response".
9. The renderer is a pure presentational component. It derives all display state from props. It does not fetch data, read from Zustand, or manage side effects. It can be used in Storybook with mock data in isolation.
10. Rendering a 200-field, 3-level-deep JSON object completes in under 100ms on a mid-range laptop. For responses larger than 100KB (up to the 500KB limit from S-046), progressive rendering is acceptable: render the first two levels immediately, then render deeper levels on the next frame via `requestIdleCallback` or `startTransition`. The 100ms budget applies to the initial visible render, not the full tree. Do not add `react-virtual` prematurely — but add a code comment noting it as the escape hatch for pathological cases.
11. The Document Renderer is wrapped in a dedicated `<RendererErrorBoundary>` (distinct from the step-level error boundary). The fallback UI shows: "Could not render this response" with options to "View raw JSON" (shows the raw `JSON.stringify(data, null, 2)` in a `<pre>` block) and "Try a different response". This is critical because the renderer processes arbitrary `data: unknown` and applies multiple formatters recursively — malformed input that passes `JSON.parse` could trigger unexpected formatter behavior.

### Technical Notes

- Implement `formatFieldName(key: string): string` and `formatValue(key: string, value: unknown): string | ReactNode` as pure functions in `src/lib/rendererFormatters.ts`. Unit test these functions exhaustively — they are the core logic of the renderer.
- JSONPath generation for `data-jsonpath`: implement a recursive `buildJsonPath(key: string | number, parentPath: string): string` function. Array element paths: `$.orders[0].id`. Object paths: `$.customer.name`. Root: `$`. JSONPath computation (`buildJsonPath`) is performed once during the recursive render pass, not recomputed on every re-render. Paths are stored as `data-jsonpath` attributes on DOM elements and read from the DOM on click events — they are never stored in React state.
- Do not use a third-party JSON-to-HTML library. The custom renderer is a core product differentiator and must be fully controlled.
- Collapsible sections: use a `<details>`/`<summary>` element pattern for native accessibility support, styled with Tailwind to match the design. Do not implement custom open/close logic with `useState` for this. The `depth` prop is passed through recursion. `<details open={depth < 2}>` controls the initial expansion. The `depth` starts at 0 for the root object.
- Table rendering for arrays of objects: cap column count at 8; additional columns are hidden with a "Show more columns" control. On mobile (`< 768px`), wrap the `<table>` in a `<div>` with `overflow-x: auto` for horizontal scrolling and cap visible columns at 3 (not 8) with "Show more columns" control.
- `React.memo` wraps the inner `<RendererValue>` component to prevent re-renders of unchanged subtrees when `selectedPaths` changes. Alternatively, if the React Compiler is configured, document that memo is not needed.
- Consider using `<Activity mode="hidden">` (React 19.2) to keep the Document Renderer mounted but hidden when navigating to later builder steps. This avoids re-rendering the full JSON tree when the user navigates back to the field mapping step. This is an optimization, not a requirement for initial implementation.

---

## S-048: Click-to-Select Field Mapping

**Story ID:** S-048
**Title:** Click-to-select field mapping
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-047 (document renderer must be implemented; field selection only makes sense on its rendered output)

### Description

As a user, I want to click on any value in the rendered API response and have it automatically added to the list of fields I'm exposing through my MCP tool, so that I can define my tool's output schema without writing JSONPath expressions or configuration files.

### Acceptance Criteria

1. Clicking any rendered value in the Document Renderer (S-047) adds that field to a "Selected fields" panel that appears alongside or below the renderer. The click handler receives the `data-jsonpath` attribute of the clicked element and the human-readable field name from the adjacent label. The panel has `data-testid="selected-fields-panel"`.
2. Each selected field appears as a "chip" row in the Selected Fields panel containing:
   - An editable text input for the field name (pre-filled with the formatted key name from S-047 AC 2, e.g. "Customer Name"). `data-testid="field-name-input"`.
   - A type badge showing the inferred type (`string`, `number`, `boolean`, `date`, `array`). Inferred from the actual value, not the key name. `data-testid="field-type-badge"`.
   - An example value shown as muted truncated text (max 40 characters, full value in `title` attribute). `data-testid="field-example-value"`.
   - A "Remove" button (×) with `aria-label="Remove [field name] field"`. `data-testid="field-remove-btn"`.
3. Clicking the same value a second time removes the field from the Selected Fields panel (toggle behavior). Clicking a second distinct element with the same JSONPath as an already-selected field also removes it (they refer to the same field).
4. Duplicate field names are prevented: if the user edits a field name input to a name that already exists in the Selected Fields panel, the input gets a red ring and inline error "A field named '[name]' already exists." The conflict is resolved on the duplicate, not the original.
5. The field name input accepts only: letters, numbers, spaces, underscores, hyphens. Max length: 64 characters. Validation runs on every keystroke (no debounce needed — it's a short input). Invalid characters are rejected silently (the character simply does not appear). An empty field name on blur reverts to the last valid value.
6. Fields in the Selected Fields panel can be reordered via drag-and-drop AND keyboard. **Pointer reorder:** Use the HTML Drag and Drop API. Each chip has a drag handle icon (six dots) on the left with `aria-label="Drag to reorder"`. The drag target drop zone is the full chip row. A visual insertion indicator (2px brand-colored line) shows where the dragged item will land. **Keyboard reorder:** When the drag handle is focused, Arrow Up/Down moves the item. Enter/Space activates "drag mode" (visual indicator shows), Arrow Up/Down repositions, Enter/Space confirms, Escape cancels. An `aria-live="polite"` region announces position changes: "[Field name] moved to position [N] of [total]". **Touch devices:** The HTML DnD API does not fire on touch screens. On touch devices, the drag handle is hidden and replaced with explicit "Move up" / "Move down" icon buttons. Detect touch capability via `matchMedia('(pointer: coarse)')`.
7. The Selected Fields panel shows a count: "N fields selected" in the panel header. When zero fields are selected, the panel shows an empty state: "Click any value in the response to add it as a tool output field." in muted text. The count and empty state are `data-testid="field-count"` and `data-testid="fields-empty-state"` respectively.
8. All selected fields (JSONPath, display name, type, example, order) are stored in `builderStore.selectedFields: SelectedField[]`. The order in the array matches the visual order in the panel.
9. The "Continue" button advancing to tool naming (S-050) is disabled when `selectedFields.length === 0`. It displays a tooltip on hover: "Select at least one field to continue." when disabled.
10. The layout is a two-column split on desktop (`>= 1280px`): Document Renderer on the left (60% width), Selected Fields panel on the right (40% width). On tablet and mobile, the Selected Fields panel renders below the Document Renderer as a full-width section.
11. A "Clear all" button appears in the Selected Fields panel header when 2 or more fields are selected. Clicking it shows a confirmation: "Remove all [N] selected fields?" with "Clear all" (destructive) and "Cancel" buttons. `data-testid="clear-all-fields-btn"`.

### Technical Notes

- Selected field state is the source of truth in Zustand. The Document Renderer's `selectedPaths` prop is derived from `builderStore.selectedFields.map(f => f.jsonPath)` and passed down. No prop drilling through intermediate components — use a selector hook `useSelectedPaths()`.
- The `useSelectedPaths()` selector must return a **stable** `Set` reference. Implement as a Zustand computed selector with `useMemo` or `useRef`-based memoization. Creating `new Set(selectedFields.map(f => f.jsonPath))` on every render causes the `DocumentRenderer` to re-render unnecessarily (Set reference equality fails even when contents are identical). Compare the array contents and only create a new Set when paths actually change.
- Drag and drop: implement as a standalone `useDragToReorder<T>(items: T[], onReorder: (newItems: T[]) => void)` hook. It returns event handlers for `onDragStart`, `onDragOver`, `onDrop`, and `onDragEnd` that are spread onto each item and the container. The hook manages `draggedIndex` and `dropIndex` in local state.
- Type inference for chips: `typeof value === 'string' && /^\d{4}-\d{2}-\d{2}/.test(value)` → `date`. `Array.isArray(value)` → `array`. Otherwise, `typeof value` for string/number/boolean.
- The `SelectedField` interface:
  ```typescript
  interface SelectedField {
    jsonPath: string;
    displayName: string;
    inferredType: 'string' | 'number' | 'boolean' | 'date' | 'array';
    exampleValue: string;
  }
  ```

---

## S-049: Array Path Normalization

**Story ID:** S-049
**Title:** Array path normalization
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** S-048 (array normalization is a post-selection enrichment step applied to selected fields that have array contexts)

### Description

As a user, when I click on a value inside an array element (e.g., the `id` field of the first order in an orders list), I want the system to automatically generalize the path to cover all items in the array, not just the zeroth element, so that my MCP tool returns data for all array items rather than a single hardcoded one.

### Acceptance Criteria

1. When the user clicks a value whose computed JSONPath contains an array index (e.g., `$.orders[0].id`), the system detects the array context and opens a confirmation dialog before adding the field to the Selected Fields panel. The dialog has `data-testid="array-normalization-dialog"`.
2. The dialog displays:
   - Heading: "This value is inside a list"
   - Explanation: "The field 'Id' is inside `orders`, which contains [N] items. Do you want to select this field for all items, or just the first one?"
   - A preview section showing the values of this field across all array items (up to 5 shown, "and N more" for larger arrays). Example: `1001, 1002, 1003` for an `id` field.
   - Two action buttons: "Select for all items" (primary, `data-testid="normalize-array-confirm"`) and "Select first item only" (secondary, `data-testid="normalize-array-decline"`).
3. Clicking "Select for all items" normalizes the JSONPath: replace all `[N]` array index segments with `[*]`. `$.orders[0].id` becomes `$.orders[*].id`. The field is added to Selected Fields with the normalized path. The type badge shows `array` for normalized paths.
4. Clicking "Select first item only" adds the field with the original indexed path (`$.orders[0].id`). The type badge shows the scalar type of that value (e.g., `number`).
5. Nested arrays (e.g., `$.orders[0].items[0].sku`) are handled. Both array segments are normalized when "Select for all items" is chosen: `$.orders[*].items[*].sku`. The dialog lists both array contexts.
6. Mixed-type arrays (arrays where elements have different types) show a warning in the dialog: "Note: this array contains mixed types. The preview shows the first 5 items." The normalization still proceeds if the user confirms.
7. After normalization, clicking an array element field that is already selected (by its normalized path) toggles it off, consistent with S-048 AC 3 toggle behavior.
8. The array index detection regex: `/\[(\d+)\]/g`. This matches standard JSONPath array indices. It must not match `[*]` (already normalized) or `['key']` (string-keyed object notation, not arrays).
9. Path normalization is implemented as a pure function `normalizeArrayPath(jsonPath: string): string` in `src/lib/jsonPathUtils.ts`, with unit tests covering: no array in path, single array, nested arrays, path ending at array element itself (e.g., `$.orders[0]`).
10. If the JSON response has no arrays, this story's UI never appears. The dialog is only triggered by the click handler in S-048 when array indices are present in the computed path.

### Technical Notes

- Array element count for the dialog preview: traverse the original parsed response object using the JSONPath up to the array segment (e.g., `$.orders`) and read `.length`. Use a simple recursive object traversal function — do not add a JSONPath evaluation library for this use case.
- The dialog is a modal overlay, not an inline expand. Use `<dialog>` element with `showModal()` for native browser focus trapping. Style with Tailwind.
- Preview values: map `response.orders.map(o => o.id)` (conceptually). In practice, evaluate the path up to the field for each array item.

### Dialog focus management

11. When the dialog closes (either action), focus returns to the element in the Document Renderer that triggered the dialog (the clicked value). This is implemented by storing a ref to the trigger element before opening the dialog and calling `.focus()` on dialog close.
12. When clicking a value that is inside an array AND is already selected (by its normalized path), the dialog is NOT shown — the field is simply toggled off (removed), consistent with S-048 AC 3 toggle behavior. The dialog only appears for new selections.

---

## S-050: Tool Naming and Description Form

**Story ID:** S-050
**Title:** Tool naming and description form
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-048 (at least one field must be selected before the user reaches this step)

### Description

As a user, I want to give my MCP tool a name and optional description so that Claude knows what this tool does and when to use it, and so that I can recognize it in my dashboard.

### Acceptance Criteria

1. The tool naming form is step 5 of the builder flow (after URL, auth, test, field mapping). It renders with `data-testid="tool-naming-form"`.
2. A "Tool name" input field (`data-testid="tool-name-input"`) with:
   - Label: "Tool name"
   - Helper text: "Used by Claude to identify and call this tool. Lowercase letters, numbers, and underscores only."
   - Validation rules enforced on every keystroke and on blur:
     - Allowed characters: `[a-z0-9_]` only. Any other character typed is rejected (does not appear in the field).
     - Minimum length: 3 characters.
     - Maximum length: 50 characters.
     - Cannot start or end with underscore.
     - Cannot contain consecutive underscores (`__`).
   - If all validation passes, no error is shown. If any rule fails on blur, the specific rule's error message appears inline below the input.
3. An auto-suggested name is pre-populated in the tool name field when this step is first reached. The suggestion is derived from the URL path:
   - Take the URL path (e.g., `/customers/{customer_id}/orders`)
   - Strip path parameters: `/customers/orders`
   - Strip leading slash, replace remaining slashes with underscores: `customers_orders`
   - Lowercase: `customers_orders`
   - Truncate to 50 characters.
   - If the result is shorter than 3 characters, use `my_tool` as fallback.
   The user can edit the suggestion freely.
4. A "Description" textarea (`data-testid="tool-description-input"`) with:
   - Label: "Description (optional)"
   - Helper text: "Explain what this tool does in plain English. Claude uses this to decide when to call it."
   - `maxLength={500}`. A character counter shows "N / 500" below the textarea, updating on every keystroke. The character counter has `aria-live="polite"` so screen readers announce the count as it approaches the limit (announce at 400, 450, 475, and 500 characters).
   - No validation error — the field is optional.
5. A preview section below the form (`data-testid="tool-preview"`) shows how the tool will appear in Claude Desktop's tool picker:
   - A mock Claude Desktop UI element (styled to resemble Claude's tool card UI) showing the tool name in monospace font and the description in regular text.
   - Updates in real time as the user types.
   - If description is empty, the preview shows "[No description provided]" in muted text.
6. The "Continue" button (advancing to the deploy review step, EPIC-04 S-060) is enabled only when the tool name passes all validation rules. It has `data-testid="tool-naming-continue"`.
7. Tool name uniqueness: if the user's account already has a server with the same tool name, show a warning (not an error): "You already have a tool named '[name]'. Consider a different name to avoid confusion." The warning does not block deployment. Check via `GET /api/v1/servers?toolName=[name]` (Tanstack Query, debounced 500ms after last keystroke).
8. All form values are stored in `builderStore.toolConfig: { name: string; description: string }`.

### Technical Notes

- The character rejection on keystroke is implemented via a controlled input's `onChange` handler that strips disallowed characters before updating state. Do not use `onKeyDown` with `preventDefault` — it breaks paste and mobile IME input.
- Tool name uniqueness check: use `useQuery` with `enabled: toolName.length >= 3 && isValidToolName(toolName)`. Debounce the `toolName` value with a **`setTimeout`-based debounce hook** (`useDebounce(toolName, 500)`) before passing it to the query key. Do NOT use `useDeferredValue` — it defers rendering, not network calls, and will not prevent the query from firing on every keystroke.
- The Claude Desktop mock preview: use a static mockup div, not an iframe or external dependency. It is a design component, not a real integration.

---

## S-051: Request Body Builder (POST/PUT/PATCH)

**Story ID:** S-051
**Title:** Request body builder for POST/PUT/PATCH methods
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** S-043 (method selection determines when this section is visible), S-045 (request body is part of the test call payload)

### Description

As a user integrating with a POST, PUT, or PATCH API endpoint, I want to define the request body my MCP tool will send so that Claude can provide all required parameters when invoking the tool and the API receives a well-formed request.

### Acceptance Criteria

1. The request body builder section is only visible when the selected HTTP method is `POST`, `PUT`, or `PATCH` (as set in S-043). It renders inline in the builder flow between the URL/params section and the auth section. It has `data-testid="request-body-builder"`.
2. The section contains a JSON editor. For MVP, this is a styled `<textarea>` with monospace font, `spellCheck={false}`, line numbers (implemented as a side-by-side `<div>` with pre-rendered line counts, not a library), and `data-testid="request-body-editor"`. CodeMirror or Monaco Editor are P2 enhancements.
3. JSON syntax validation runs on `onChange` (debounced 400ms). Invalid JSON shows an inline error below the editor: "Invalid JSON: [error message] at line [N]." using the same line-number extraction approach as S-046 AC 3. Valid JSON shows a green "Valid JSON" badge adjacent to the editor header.
4. Template variable support: the user can write `{{param_name}}` placeholders in the JSON body. Example: `{ "customer_id": "{{customer_id}}", "limit": {{limit}} }`. Template variables are detected by the regex `/\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g`. Each detected variable name is extracted and displayed in an "Input parameters" list below the editor, showing: variable name, inferred type (`string` if quoted in JSON, `number` if unquoted), an editable label for the Claude-facing parameter name, and a "Required / Optional" toggle.
5. The input parameters derived from template variables are merged with the path/query parameters from S-043 to form the complete MCP tool input schema. There must be no duplicate names between the two sources. If a template variable name collides with a path/query param name, highlight the collision with an amber warning: "Parameter name '[name]' already exists as a path/query parameter. Rename one of them."
6. A "Load example body" helper link opens a small dropdown with common JSON body patterns: `{ "id": "{{id}}" }`, `{ "data": { "key": "{{value}}" } }`, and `{ "query": "{{search_term}}", "limit": {{limit}} }`. Selecting a pattern populates the editor (overwriting current content after a "Replace current content?" confirmation if the editor is non-empty). `data-testid="load-example-body-btn"`.
7. The request body content (raw JSON string with template variables) is stored in `builderStore.requestBody: string | null` (null when method is GET/DELETE, empty string when method is POST/PUT/PATCH and user has not entered anything).
8. When the test call is executed (S-045), the request body is included in the proxy payload. Template variables in the body are substituted with the example values entered by the user in the parameter inputs section of S-045.
9. If the method changes from POST to GET (user switches the dropdown in S-043), the request body section hides and `builderStore.requestBody` is set to `null`. If the method changes back to POST, the section reappears with the previously entered content restored (not cleared).
10. The editor has a minimum height of 8 lines and a maximum height of 24 lines. Below 8 lines it does not collapse; above 24 lines a vertical scrollbar appears inside the editor rather than the page scrolling.
11. Template variable name collisions with path/query params display an amber warning but do NOT block the Continue button. The warning is informational — the user may intentionally want the same parameter name to serve dual purposes. The Continue button tooltip shows "1 parameter name collision — review before deploying" when a collision exists.

### Technical Notes

- Line numbers for the textarea: absolute position a `<div>` to the left of the `<textarea>`. Compute line count from `value.split('\n').length`. Re-render line numbers on every state change. This is not pixel-perfect for wrapped lines but is acceptable for MVP given a fixed-width monospace font and `white-space: pre` / `overflow-x: auto` on the textarea.
- Template variable extraction runs as a derived computation from the raw body string. Do not store the extracted variables in Zustand — derive them with `useMemo` from `builderStore.requestBody`.
- The complete tool input schema (path params + query params + body template vars) is assembled in a selector `useToolInputSchema()` that reads from `builderStore` and returns a `ToolParameter[]` array. This selector is the single source of truth for the MCP tool's `inputSchema` used in the deploy review step (EPIC-04).
- JSON validation and line-number extraction use the shared `parseJsonWithLineNumbers()` utility from `src/lib/jsonValidator.ts` (same as S-046). Do not reimplement.
- The textarea uses an uncontrolled pattern (same as S-046) for consistency and performance. Line numbers are re-rendered via a `requestAnimationFrame` callback tied to the textarea's `scroll` and `input` events.
