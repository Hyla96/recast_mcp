# Epic 04: Deployment and Operations

**Product:** Dynamic MCP Server Builder
**Epic ID:** EPIC-04
**Date:** 2026-03-28
**Status:** Ready for Engineering
**Tech Stack:** React 19 + TypeScript + Vite + TailwindCSS + Tanstack Query + Zustand + React Router

**Scope:** Everything that happens after the builder flow is complete. This epic owns: the deploy confirmation screen, connection config blocks, the in-browser playground, the server detail page, real-time status monitoring, Bearer token management, server deletion, responsive layout validation, comprehensive error and empty states, and keyboard/accessibility compliance.

**Definition of Done (epic-level):** A deployed server is reachable via its generated MCP URL, the user can copy connection config for at least one client (Claude Desktop), test the tool in-browser via the playground, and manage or delete the server — all without encountering unhandled errors or inaccessible UI elements.

---

## Story Index

| Story ID | Title | Points | Priority |
|----------|-------|--------|----------|
| S-060 | Deploy flow — review and confirm | 5 | P0 |
| S-061 | Connection config blocks | 3 | P0 |
| S-062 | Playground panel | 8 | P0 |
| S-063 | Server detail page | 8 | P1 |
| S-064 | Server status monitoring | 5 | P1 |
| S-065 | Bearer token management | 3 | P0 |
| S-066 | Server deletion with confirmation | 3 | P1 |
| S-067 | Responsive design and mobile | 5 | P1 |
| S-068 | Error states and empty states | 5 | P0 |
| S-069 | Keyboard shortcuts and accessibility | 5 | P1 |

**Epic total:** 50 points

---

## S-060: Deploy Flow — Review and Confirm

**Story ID:** S-060
**Title:** Deploy flow — review and confirm
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** S-050 (tool name required), S-048 (selected fields required), S-044 (auth config required), S-043 (URL and method required). This story is the final screen of EPIC-03's builder flow and the gateway to EPIC-04.

### Description

As a user who has completed the builder form, I want to see a clear summary of everything I've configured before I deploy, so that I can catch mistakes before the server goes live and understand exactly what is being created on my behalf.

On confirmation, I want immediate feedback — a success screen with my server's live URL — so that the "90-second flow" promise feels real and the transition from builder to live infrastructure is viscerally fast.

### Acceptance Criteria

1. The review screen is the final step of `/servers/new`. It has `data-testid="review-screen"` and displays all configuration in read-only summary form. No editing on this screen — the "Back" button returns to the tool naming step (S-050) with all state intact.
2. The review screen displays the following sections, each as a labeled card:
   - **Endpoint:** HTTP method badge + full URL. `data-testid="review-endpoint"`.
   - **Authentication:** auth type label only (e.g., "Bearer Token", "API Key — Header", "Basic Auth", "None"). The credential value is never shown. `data-testid="review-auth"`.
   - **Tool:** tool name in monospace font, description in regular text (or "[No description]" in muted text if empty). `data-testid="review-tool"`.
   - **Fields:** a table listing all selected fields with columns: Field Name, Type, JSONPath. `data-testid="review-fields"`. If the server is unverified (S-046), display a yellow "Sample response — not live-tested" banner above this section.
3. The "Deploy" button (`data-testid="deploy-btn"`) is the primary action. It is full-width on mobile, right-aligned on desktop. It has an icon (rocket or similar) and label "Deploy server".
4. Clicking "Deploy" sends `POST /api/v1/servers` with the complete server configuration. While the request is in-flight:
   - The "Deploy" button is disabled and its label changes to a spinner + "Deploying..."
   - A progress message below the button cycles through: "Provisioning server..." → "Configuring authentication..." → "Verifying endpoint..." → "Almost there..." at 1-second intervals. These are cosmetic — they do not reflect real backend progress.
   - The "Back" button is disabled during deployment.
5. **Success:** the review screen transitions to a success screen (same route, different view state). The success screen has `data-testid="deploy-success-screen"` and displays:
   - A green checkmark icon (animated, drawing in over 400ms using CSS stroke animation).
   - Heading: "Your server is live!"
   - The generated MCP server URL in a monospace code block with a copy button. `data-testid="server-url-display"`. Clicking copy uses `navigator.clipboard.writeText()` and shows a transient "Copied!" label for 2 seconds.
   - Two CTA buttons: "Open Playground" (navigates to `/servers/:id/playground`, `data-testid="open-playground-btn"`) and "Go to Dashboard" (navigates to `/dashboard`).
   - The connection config blocks (S-061) are rendered below the CTAs.
6. **Failure:** if `POST /api/v1/servers` returns a non-2xx response, the deploy button re-enables, the progress message is replaced with a red error banner: "Deployment failed: [error message from API]." with a "Try again" button that re-submits the same payload. The user's configuration is not lost.
7. The `POST /api/v1/servers` response must return at minimum: `{ id: string; url: string; bearerToken: string }`. The `bearerToken` is stored in Zustand (`builderStore.deployResult.bearerToken`) for display in S-065 and is not persisted to `localStorage`. It is the only time the plain-text token is available in the client.
8. The deploy mutation is a Tanstack Query `useMutation`. On success, `queryClient.invalidateQueries(['servers'])` is called to refresh the dashboard list. The mutation is idempotent on retry (the backend must handle duplicate deploy requests gracefully — document this as a backend contract).
9. The success screen is not accessible by navigating back (browser back button). After deploy, the browser history entry for `/servers/new` is replaced with `/servers/:id` so the back button returns to `/dashboard`.
10. Deploying takes under 3 seconds in the success path (backend SLA). The frontend does not enforce this SLA but the cosmetic progress messages are timed to complete by 2.5 seconds, creating a smooth transition.

### Technical Notes

- Use `useNavigate` with `{ replace: true }` after successful deployment to prevent back-navigation to the builder.
- The green checkmark animation: use an SVG `<path>` with `stroke-dasharray` and `stroke-dashoffset` animated via a CSS `@keyframes` rule in Tailwind's `@layer utilities`. No JavaScript animation library.
- Progress message cycling: use `setInterval` inside a `useEffect` that cleans up when the mutation settles. Clear interval on unmount.
- The `bearerToken` from the deploy response is the only entry point for S-065's "token displayed once" requirement. The store slice must be cleared when the user navigates away from the success screen (use `useEffect` cleanup or a route `loader`/`action` to wipe it).

---

## S-061: Connection Config Blocks

**Story ID:** S-061
**Title:** Connection config blocks
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-060 (deploy success — server URL and Bearer token are required), S-065 (Bearer token management — token display feeds into config blocks)

### Description

As a user who has just deployed a server, I want ready-to-paste configuration snippets for my AI clients (Claude Desktop, Cursor, VS Code Copilot) so that I can connect my MCP server to my tools in under 60 seconds without consulting documentation.

### Acceptance Criteria

1. Connection config blocks render on the deploy success screen (S-060) and on the server detail page (S-063). They are a standalone `<ConnectionConfigBlocks serverId={id} serverUrl={url} />` component that accepts these two props and is not coupled to builder state. `data-testid="connection-config-blocks"`.
2. Three client configurations are provided, each in a collapsible section. Default state: Claude Desktop is expanded, Cursor and VS Code Copilot are collapsed. Each section has a client name heading and a toggle chevron.
3. Each config block contains:
   - A short instruction sentence: e.g., "Add this to your `claude_desktop_config.json` file."
   - A syntax-highlighted code block (JSON for all three clients). The syntax highlighting is a lightweight custom implementation: strings in green, keys in blue, booleans in amber — implemented as a single regex-replace pass on the JSON string, not a full parser. `data-testid="config-code-block-[claude|cursor|vscode]"`.
   - A "Copy" button at the top-right of the code block. Clicking copies the raw (unstyled) JSON string. Shows "Copied!" for 2 seconds. `data-testid="copy-config-btn-[claude|cursor|vscode]"`.
4. The Claude Desktop config snippet format:
   ```json
   {
     "mcpServers": {
       "[tool_name]": {
         "url": "[server_url]",
         "transport": "http",
         "headers": {
           "Authorization": "Bearer [bearer_token]"
         }
       }
     }
   }
   ```
   Where `[tool_name]` is the tool name from S-050, `[server_url]` is the live server URL from deploy, and `[bearer_token]` is the user's Bearer token (masked as `••••••••[last4]` if viewed after initial deploy, plain text on the success screen only).
5. The Cursor config snippet format mirrors the Claude Desktop format but uses Cursor's documented MCP config key (`mcpServers` under `.cursor/mcp.json`). The instruction text references `.cursor/mcp.json`.
6. The VS Code Copilot config snippet format uses the VS Code MCP extension format. The instruction text references the VS Code settings JSON path.
7. Config format versions: the config format for each client is hardcoded to the latest known format at build time. A comment in the source file (`// Last verified: YYYY-MM-DD`) documents the version. Format updates are a maintenance task, not a runtime concern. The component does not fetch config format specs from an external source.
8. On the server detail page (S-063), if the Bearer token has been masked (post-initial-deploy), the config blocks show the masked token and a "Regenerate token to get a new config" note. On the deploy success screen, the plain-text token is shown once (sourced from `builderStore.deployResult.bearerToken`).
9. Each section's expanded/collapsed state is persisted in `localStorage` under the key `"mcp-config-[clientName]-expanded"`. Returning to the server detail page restores the last-used state.
10. All three code blocks are keyboard accessible: the Copy button is Tab-reachable, activatable with Enter/Space, and announces "Copied to clipboard" to screen readers via an `aria-live="polite"` region.

### Technical Notes

- The Bearer token in config blocks sources from two places: (a) `builderStore.deployResult.bearerToken` (deploy success screen, plain text) and (b) a masked representation `"••••••••" + token.slice(-4)` on the server detail page. The component accepts an optional `bearerToken?: string` prop; absence of the prop triggers the masked placeholder.
- Do not use `react-syntax-highlighter` or Prism for this use case. The regex-based approach is sufficient, saves bundle size, and avoids version conflicts.

---

## S-062: Playground Panel

**Story ID:** S-062
**Title:** Playground panel
**Priority:** P0
**Estimated Effort:** 8 points
**Dependencies:** S-060 (server must be deployed to have a tool to test), S-047 (document renderer used to display results), S-061 (playground is the immediate next step after reading connection configs)

### Description

As a user, I want to test my deployed MCP tool in-browser without setting up Claude Desktop or Cursor, so that I can immediately verify the tool works correctly with real parameter values and see exactly what Claude will receive in response.

The playground is the "aha moment" for new users — seeing their API response rendered in the MCP tool format, in-browser, in seconds, confirms that the platform actually works.

### Acceptance Criteria

1. The `/servers/:id/playground` route renders the playground panel. The route is accessible from the deploy success screen ("Open Playground" button) and from the server detail page's navigation. `data-testid="playground-panel"`.
2. On mount, the playground fetches the server's tool definition from `GET /api/v1/servers/:id/tool-definition`, which returns:
   ```typescript
   interface ToolDefinition {
     name: string;
     description: string | null;
     inputSchema: {
       type: 'object';
       properties: Record<string, { type: string; description?: string }>;
       required: string[];
     };
   }
   ```
   While fetching, show a skeleton for the parameter form. On error, show "Failed to load tool definition" with a retry button.
3. The tool definition section renders at the top of the playground:
   - Tool name in a monospace heading.
   - Description in regular text (or "[No description]" in muted text).
   - A "Tool schema" collapsible section (collapsed by default) showing the raw `inputSchema` JSON in a code block. `data-testid="tool-schema-code"`.
4. Below the tool definition, a parameter form renders one input field per property in `inputSchema.properties`. Each input:
   - Has a label equal to the property key (formatted with the same Title Case logic as S-047 AC 2).
   - Has a helper text from `inputSchema.properties[key].description` if present.
   - Has a red asterisk and `aria-required="true"` if the key is in `inputSchema.required`.
   - Has `type="number"` if the schema type is `number`, otherwise `type="text"`.
   - `data-testid="playground-param-[key]"`.
5. A "Run tool" button (`data-testid="playground-run-btn"`) submits the parameter form. Clicking it:
   - Validates that all required fields are non-empty. If any required field is empty, focus the first invalid field and show an inline error. Do not submit.
   - Sends `POST /api/v1/servers/:id/invoke` with body `{ parameters: { [key]: value, ... } }`.
   - Disables the button and shows spinner + "Running..." during the call.
6. **Success result:** the result is rendered using the Document Renderer component (S-047) in a results panel below the form. A "Result" heading with a green "Success" badge and the upstream response status code. `data-testid="playground-result"`. The rendered result persists until the next run.
7. **Error results — differentiated display:**
   - **Timeout (>30s):** red banner "The tool call timed out after 30 seconds. The upstream API may be slow or unreachable." with a "Try again" button.
   - **Upstream API error (4xx/5xx):** red banner "The upstream API returned [status]: [message]." with the raw response body in a collapsed code block.
   - **Transform error (field extraction failed):** amber banner "The tool ran successfully, but some fields could not be extracted. Raw response is shown below." Render the raw JSON response in a code block alongside whatever partial result was extracted.
   - **Platform error (5xx from platform API):** red banner "Something went wrong on our end. Please try again in a moment." with `data-testid="playground-platform-error"`.
8. Multiple runs are supported without page refresh. Each "Run tool" click replaces the previous result. There is no run history in this story (P2 enhancement).
9. The playground remembers the last-entered parameter values in component state (not Zustand — scoped to the playground session). Navigating to the server detail page and back clears the values.
10. The "Run tool" call is made with the user's Bearer token in the `Authorization: Bearer` header. The token is fetched via Clerk's `getToken()` (user auth token, not the server's Bearer token). The server's own Bearer token is only used by MCP clients, not by the playground's browser-side call.

### Technical Notes

- The invoke endpoint (`POST /api/v1/servers/:id/invoke`) is a platform-side proxy. It authenticates the user (via their Clerk session), retrieves the server's encrypted credentials from the database, executes the tool call against the upstream API, applies the field transform, and returns the MCP-shaped result.
- The playground's `useMutation` returns a discriminated union result type analogous to S-045's `TestCallResult`, plus a `transform_error` variant.
- Parameter form state: use `react-hook-form` for this form (it is the only form complex enough to justify it — required validation, type coercion, focus management). If `react-hook-form` is not yet in the project, add it in this story.
- The playground route is a full page, not a panel overlay, to allow deep linking and browser history navigation.

---

## S-063: Server Detail Page

**Story ID:** S-063
**Title:** Server detail page
**Priority:** P1
**Estimated Effort:** 8 points
**Dependencies:** S-060 (server must exist), S-061 (connection configs embedded), S-062 (playground link), S-064 (status data), S-065 (token management UI embedded)

### Description

As a user, I want a single page where I can see everything about a deployed server: its current health, recent call history, connection instructions, and controls to edit or delete it, so that I can operate and maintain my MCP server without leaving the platform.

### Acceptance Criteria

1. The `/servers/:id` route fetches full server details from `GET /api/v1/servers/:id`. The response shape:
   ```typescript
   interface ServerDetail {
     id: string;
     name: string;
     status: 'active' | 'degraded' | 'error' | 'inactive';
     toolName: string;
     description: string | null;
     endpointUrl: string;
     method: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';
     authType: 'none' | 'bearer' | 'apikey' | 'basic';
     selectedFields: SelectedField[];
     isUnverified: boolean;
     createdAt: string;
     updatedAt: string;
     stats: {
       callsLast24h: number;
       errorRateLast24h: number; // 0-1
       lastCallAt: string | null;
       lastSuccessAt: string | null;
     };
   }
   ```
2. The page header shows: server name (h1), status badge (from S-064), a "Playground" button linking to `/servers/:id/playground`, and a kebab menu (three dots) containing "Edit server" and "Delete server" actions. `data-testid="server-detail-header"`.
3. **Stats row:** four stat cards in a responsive row:
   - "Calls (24h)" with the count formatted via `Intl.NumberFormat`. `data-testid="stat-calls-24h"`.
   - "Error rate (24h)" as a percentage (e.g., "3.2%"). Red text if `> 10%`, amber if `> 1%`, green if `<= 1%`. `data-testid="stat-error-rate"`.
   - "Last call" as relative time. `data-testid="stat-last-call"`.
   - "Last success" as relative time. `data-testid="stat-last-success"`.
4. **Recent calls table:** a table showing the last 50 calls, fetched from `GET /api/v1/servers/:id/calls?limit=50`. Columns: Timestamp (absolute, `MMM D, YYYY HH:mm:ss`), Status (green "Success" or red "Error" badge), Latency (in ms, formatted as "143 ms"), Parameters (truncated JSON of the call's input parameters, expandable on click). `data-testid="recent-calls-table"`. If no calls have been made, show "No calls yet" in an empty state row.
5. **Error rate chart:** a simple 24-hour bar chart showing error rate per hour. 24 bars, each representing one hour. Bar color: green if 0% errors, amber if 1-50%, red if >50%. Bars use CSS heights (percentage of max) — no chart library. X-axis: "6h ago", "3h ago", "1h ago", "Now". `data-testid="error-rate-chart"`. Data fetched from `GET /api/v1/servers/:id/metrics?range=24h`.
6. **Connection config section:** embeds the `<ConnectionConfigBlocks />` component from S-061. `data-testid="server-detail-config"`.
7. **Token management section:** embeds the Bearer token management UI from S-065. `data-testid="server-detail-token"`.
8. **Edit mode:** clicking "Edit server" in the kebab menu renders an edit drawer or expands an inline edit section. In edit mode, the user can modify: endpoint URL, HTTP method, auth configuration, and selected fields. Editing navigates the user back through the relevant builder sub-steps (S-043, S-044, S-048) presented as a condensed in-page form, not as a full-page wizard. Saving sends `PATCH /api/v1/servers/:id` and triggers a hot reload of the server configuration without downtime. `data-testid="server-edit-mode"`.
9. After a successful edit save, show a toast notification "Server updated. Changes are live." and refresh the page data via `queryClient.invalidateQueries(['server', id])`.
10. All data on this page (server detail, recent calls, metrics) is loaded with Tanstack Query. Loading states use skeleton loaders for each section independently — a slow metrics fetch does not block the rest of the page from rendering.

### Technical Notes

- The edit mode re-uses the builder step components (S-043, S-044, S-048) as controlled components, passing current server config as initial values and accepting onChange callbacks. They must be designed for this dual use from the start. Add a `mode: 'create' | 'edit'` prop to each relevant component.
- Recent calls table: use `useInfiniteQuery` even though only 50 rows are shown now. This makes pagination a trivial addition.
- The error rate chart: store 24 hourly buckets as an array of `{ hour: number; errorRate: number }`. Compute bar height as `Math.round(errorRate * 100)` percent of the container's height. Render as `<div style={{ height: `${errorRate * 100}%` }} />` within a fixed-height flex container.
- The kebab menu uses a `<Popover>` built with Headless UI or a custom implementation using `useRef` + click-outside detection. Do not use a `<select>` element.

---

## S-064: Server Status Monitoring

**Story ID:** S-064
**Title:** Server status monitoring
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** S-042 (dashboard shows status badges — this story defines what drives them), S-063 (server detail page shows the same status data with more detail)

### Description

As a user, I want to see the real-time health of each of my servers at a glance — including whether it is healthy, degraded, or unreachable — so that I can respond to problems before my users or Claude agents are affected.

### Acceptance Criteria

1. Server status is modeled as a discriminated union:
   ```typescript
   type ServerStatus =
     | { state: 'active'; label: 'Healthy'; color: 'green' }
     | { state: 'degraded'; label: 'Degraded'; color: 'yellow' }
     | { state: 'error'; label: 'Error'; color: 'red' }
     | { state: 'inactive'; label: 'Inactive'; color: 'gray' };
   ```
   The `state` field is returned by the API. The `label` and `color` are derived client-side from the `state` value via a lookup map.
2. Status badge component: `<StatusBadge status={ServerStatus} />`. Renders a colored pill with label text and a solid circle icon. Green = `bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200`. Yellow = `bg-yellow-100 text-yellow-800`. Red = `bg-red-100 text-red-800`. Gray = `bg-gray-100 text-gray-700`. `data-testid="status-badge"`. This component is used in both the dashboard card (S-042) and the server detail header (S-063).
3. Status definitions:
   - **Active/Healthy:** fewer than 50% of calls in the last 5 minutes resulted in errors, AND the server is reachable.
   - **Degraded:** 50% or more of calls in the last 5 minutes resulted in errors, but at least one call succeeded.
   - **Error:** the server is unreachable (platform health check fails) OR 100% of calls in the last 5 minutes failed.
   - **Inactive:** no calls in the last 24 hours. Note: "inactive" is a business state, not an error state.
   The status computation is performed server-side by the backend. The frontend displays the API-returned state without recomputing it.
4. On the server detail page, the status badge is accompanied by supplementary monitoring data in a "Health" section (`data-testid="server-health-section"`):
   - "Last successful call": relative timestamp (e.g., "2 minutes ago") or "Never".
   - "Active connections": count of currently open MCP client connections. If unavailable, show "—".
   - "Upstream latency p50": median latency of upstream API calls in the last 5 minutes, in ms. If unavailable, show "—".
   - "Upstream latency p95": 95th percentile latency in the same window. If unavailable, show "—".
5. The Health section data is fetched from `GET /api/v1/servers/:id/health`. Tanstack Query polls this endpoint every 30 seconds (`refetchInterval: 30_000`). On each successful response, the status badge and all metrics update without a page reload.
6. If a WebSocket connection is available (`wss://api.example.com/ws/servers/:id`), incoming `health_update` events replace the 30-second polling for this server's health data. The component uses the `useServerStatusSocket` hook from S-042 (scoped to a single server ID).
7. When a server's status transitions from `active` to `degraded` or `error`, a toast notification appears: "Server [name] is experiencing issues." with a link to the server detail page. This notification appears regardless of which page the user is on. Toast is dismissible. `data-testid="status-alert-toast"`. Transitions in the other direction (recovery) also trigger a toast: "Server [name] is back to healthy."
8. Status transition toasts are debounced — a status that flickers between `degraded` and `error` within 60 seconds should not produce more than one toast. Implement with a `lastToastTime` ref.
9. Auto-refresh indicator: a subtle "Last updated [N] seconds ago" text below the Health section, incrementing every second via a `setInterval`. This resets to 0 on each successful poll or WebSocket event. `data-testid="last-updated-indicator"`.
10. The `<StatusBadge>` component has an `aria-label` that includes the full status description: `aria-label="Server status: Healthy"` / `aria-label="Server status: Degraded — high error rate"` / `aria-label="Server status: Error — server unreachable"`.

### Technical Notes

- Toast notification system: implement a `useToast()` hook backed by a Zustand `toastStore` slice. The store holds an array of `Toast` objects (`{ id, message, variant, link?, dismissedAt? }`). A `<ToastContainer>` component renders at the app root and observes the store. Individual page components add toasts via `useToast().add({ ... })`.
- Status transition detection: compare the previous query result to the current one in Tanstack Query's `onSuccess` callback using a `useRef` to store the last seen status.
- Do not show status toasts for servers that are currently open in the server detail page (the badge on the page is sufficient feedback). Suppress toasts when `useLocation().pathname === `/servers/${serverId}``.

---

## S-065: Bearer Token Management

**Story ID:** S-065
**Title:** Bearer token management
**Priority:** P0
**Estimated Effort:** 3 points
**Dependencies:** S-060 (token is first created at deploy time), S-063 (token management UI is embedded in the server detail page)

### Description

As a user, I want a secure way to view, copy, and rotate the Bearer token that authenticates my MCP server's clients, so that I can maintain security hygiene and recover from token compromise without rebuilding my server from scratch.

### Acceptance Criteria

1. The Bearer token management section is embedded in the server detail page (S-063) and rendered as a `<BearerTokenManager serverId={id} />` component. `data-testid="bearer-token-manager"`.
2. The token is displayed as a masked string: the first 4 characters and last 4 characters are shown, all middle characters replaced with `•`. Example: `sk_l•••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••ive`. `data-testid="token-masked-display"`.
3. There is no "reveal full token" option after initial display. The full token is shown exactly once: on the deploy success screen (S-060). After the user navigates away, the token is only recoverable by regenerating it.
4. A "Copy" button adjacent to the masked token display. Clicking it copies the full token to the clipboard using `navigator.clipboard.writeText()`. The full token is fetched from the API on copy (`GET /api/v1/servers/:id/token` — returns `{ token: string }` only to the server owner's authenticated session). On copy success, show "Copied!" for 2 seconds. `data-testid="copy-token-btn"`.
5. A "Regenerate" button with `data-testid="regenerate-token-btn"`. Clicking opens a confirmation dialog:
   - Heading: "Regenerate Bearer token?"
   - Body: "This will immediately invalidate the current token. All active MCP clients using the old token will be disconnected and will need to be updated with the new token."
   - Actions: "Cancel" and "Regenerate token" (destructive red button). `data-testid="regenerate-confirm-btn"`.
6. Confirming regeneration sends `POST /api/v1/servers/:id/token/regenerate`. On success:
   - The new token is returned in the response: `{ token: string }`.
   - The new token is displayed in full in a green success banner: "New token generated. Copy it now — it will not be shown again." with a copy button.
   - The banner auto-dismisses after the user clicks copy or after 60 seconds without copy (warning before dismiss: "Token will be hidden in 10 seconds").
   - After dismissal, the masked display resumes.
7. If regeneration fails, show an error banner "Failed to regenerate token. Please try again." The current token is not affected.
8. A "Revoke token (disable server)" link at the bottom of the section (`data-testid="revoke-token-link"`). Clicking opens a separate confirmation: "Revoke token? This will permanently disable all client connections to this server until a new token is generated." with a "Revoke" button. Revocation calls `DELETE /api/v1/servers/:id/token`. On success, the server status transitions to `inactive` and the status badge updates.
9. The token section has a help text: "Your Bearer token authenticates MCP clients (Claude Desktop, Cursor, etc.) to your server. Keep it private. If compromised, regenerate immediately." in muted small text.
10. All destructive actions (regenerate, revoke) require an explicit second confirmation step. No single-click destructive actions anywhere in the token management UI.

### Technical Notes

- The "copy full token" flow (`GET /api/v1/servers/:id/token`) should return the token only in response to a request made with the user's Clerk session token. The backend enforces ownership. The frontend always attaches the session token (automatic via the central query function from S-041 AC 6).
- The 60-second auto-dismiss for the new token banner: use `setInterval` to count down and update a `countdown` state variable displayed in the banner. Clear the interval on unmount and on copy. Warn at 10 seconds remaining.
- The full token returned on regeneration is stored in component local state only (not Zustand, not localStorage). It is cleared when the banner dismisses.

---

## S-066: Server Deletion with Confirmation

**Story ID:** S-066
**Title:** Server deletion with confirmation
**Priority:** P1
**Estimated Effort:** 3 points
**Dependencies:** S-063 (deletion is triggered from the server detail page kebab menu)

### Description

As a user, I want to permanently delete a server I no longer need, with enough friction to prevent accidental deletion, so that I can keep my account clean without worrying about fat-finger mistakes.

### Acceptance Criteria

1. "Delete server" is available in the kebab menu on the server detail page (S-063). Clicking it opens a full-screen modal overlay (not an inline confirmation). `data-testid="delete-server-modal"`.
2. The modal contains:
   - Heading: "Delete [server name]?" in bold.
   - Body (three bullet points):
     - "The server URL will be permanently decommissioned."
     - "All active MCP client connections will be dropped immediately."
     - "This action cannot be undone."
   - A text input with label: "Type [server name] to confirm" where `[server name]` is the literal server name. Placeholder: server name. `data-testid="delete-confirm-input"`.
   - Two buttons: "Cancel" (secondary) and "Delete server" (destructive, `bg-red-600`). `data-testid="delete-confirm-btn"`.
3. The "Delete server" button is disabled until the value in the confirmation input exactly matches the server's name (case-sensitive string comparison). When enabled, the button changes from `opacity-50 cursor-not-allowed` to fully interactive with a red background.
4. Clicking "Delete server" (enabled state) sends `DELETE /api/v1/servers/:id`. While in-flight:
   - The button shows spinner + "Deleting..."
   - The confirmation input and Cancel button are disabled.
5. **On success:**
   - Show a brief "Server deleted" success toast.
   - Navigate to `/dashboard` using `useNavigate('/dashboard', { replace: true })`.
   - Call `queryClient.invalidateQueries(['servers'])` to refresh the dashboard list.
6. **On failure:** re-enable the modal controls and show an error banner inside the modal: "Deletion failed: [error message]. Please try again." Do not close the modal.
7. The modal traps focus while open: Tab cycles only through modal elements. Pressing Escape closes the modal (equivalent to clicking "Cancel"). The modal is implemented with `<dialog>` + `showModal()` for native focus trapping and Escape handling.
8. On modal open, focus is placed on the confirmation text input, not the "Delete" button.
9. Closing the modal (Cancel or Escape) resets the confirmation input value. The modal can be reopened and will start fresh.
10. The deletion flow also triggers: server credential cleanup (backend responsibility — document as a backend contract: `DELETE /api/v1/servers/:id` must schedule credential deletion within 24 hours) and Bearer token revocation (backend responsibility — tokens must be invalidated synchronously before the 200 response is returned).

### Technical Notes

- The confirmation input comparison: `userInput.trim() === serverName` (trim whitespace but do not lowercase — deletion must be case-sensitive to create meaningful friction).
- `<dialog>` elements do not need a custom backdrop implementation — use `::backdrop` pseudo-element styled with `bg-black/60`.
- The `useMutation` for deletion has `onSuccess` that handles navigation. This avoids a race between toast display and navigation — call `addToast()` before `navigate()`.

---

## S-067: Responsive Design and Mobile

**Story ID:** S-067
**Title:** Responsive design and mobile
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** All stories in EPIC-03 and EPIC-04 — this is a cross-cutting validation and remediation story.

### Description

As a user on any device, I want the platform's core flows to work on my screen size so that I can manage my servers and review connection configs from a tablet or phone, even if the full builder experience requires a desktop.

### Acceptance Criteria

1. All routes and components are tested at three explicit breakpoints: mobile (`375px × 667px` — iPhone SE), tablet (`768px × 1024px` — iPad portrait), desktop (`1440px × 900px` — standard laptop). No horizontal scrollbars appear on mobile or tablet for any route.
2. **Dashboard (`/dashboard`):** fully usable on mobile and tablet. Server cards stack to one column on mobile (confirmed by S-042 AC 9). The "New server" button is always visible without scrolling on mobile. Clicking a card navigates to the server detail page (no features are hidden on mobile dashboard).
3. **Builder flow (`/servers/new`):** minimum supported width is `1024px`. Below `1024px`, the builder route renders a "Builder optimized for desktop" notice: a card in the center of the page with the message "The server builder works best on a desktop or laptop. Use a larger screen for the full experience." with a "View my servers" link. The notice is displayed instead of the builder form, not behind it. `data-testid="desktop-only-notice"`.
4. **Server detail page (`/servers/:id`):** usable on tablet (`768px`+). The stats row (S-063 AC 3) wraps to 2×2 on tablet and 1×4 on desktop. The recent calls table scrolls horizontally within its container on tablet — the table does not force page-level horizontal scroll. On mobile (< `768px`), the recent calls table is replaced with a simplified list view showing timestamp, status badge, and latency per row, without the parameters column.
5. **Playground (`/servers/:id/playground`):** usable on tablet. Parameter inputs are touch-friendly (minimum tap target: `44px × 44px` per WCAG 2.5.5). The result rendered by the Document Renderer is readable on tablet. On mobile, the playground is available (not blocked) but the document renderer is condensed: nested sections are collapsed by default and maximum object nesting displayed is 2 levels.
6. **Connection config blocks (S-061):** code blocks are horizontally scrollable within their container on all screen sizes. The "Copy" button is always visible without scrolling within the config block card. On mobile, all three client config sections start in a collapsed state to reduce initial page height.
7. The nav bar collapses to a hamburger menu on screens narrower than `768px`. The hamburger opens a slide-in drawer from the left with the same navigation items as the desktop nav. The drawer closes on Escape and on clicking outside. `data-testid="mobile-nav-drawer"`.
8. Touch targets: all buttons, links, and interactive elements have a minimum clickable area of `44px × 44px` on mobile breakpoints. This is enforced via Tailwind's `min-h-[44px] min-w-[44px]` utilities on interactive elements that would otherwise be smaller.
9. The document renderer (S-047) on mobile (`< 768px`): tables are replaced with a definition list format (field name on one line, value on the next). Arrays of objects show a "card" view (one card per array element with key-value pairs stacked vertically) instead of a horizontal table.
10. All responsive behaviors are validated in Playwright tests that set viewport size explicitly. The test suite includes at minimum: one smoke test per route at 375px width (verifying no horizontal overflow and no critical content hidden), and one test at 768px width for the builder's desktop-only notice.

### Technical Notes

- Horizontal overflow detection: add a `useEffect` in development mode that checks `document.documentElement.scrollWidth > document.documentElement.clientWidth` and logs a console warning if overflow is detected. This is a dev-only diagnostic, not a production feature.
- The builder's desktop-only notice breakpoint (`< 1024px`) is implemented as a CSS media query via Tailwind's `hidden lg:block` pattern — the notice div uses `block lg:hidden`, the builder form uses `hidden lg:block`.
- The mobile nav drawer uses a `<dialog>` element positioned at the side via CSS (`inset-y-0 left-0 max-w-xs`), not a fixed div, for proper focus trapping.

---

## S-068: Error States and Empty States

**Story ID:** S-068
**Title:** Error states and empty states
**Priority:** P0
**Estimated Effort:** 5 points
**Dependencies:** All data-fetching stories (S-042, S-045, S-062, S-063, S-064) — this story specifies and implements consistent error and empty states for all of them.

### Description

As a user, I want every part of the application to respond gracefully to missing data, loading states, and failures — with clear, actionable messages — so that I am never stuck looking at a blank screen or a raw error dump.

### Acceptance Criteria

1. **Toast notification system** (referenced by multiple stories): the `<ToastContainer>` renders at the app root. Toasts appear in the bottom-right corner on desktop, bottom-center on mobile. Maximum 3 toasts visible simultaneously; additional toasts are queued and appear as earlier ones dismiss. Each toast has: an icon (success, error, warning, info), a message, an optional action link, and a dismiss button (`×`). Toasts auto-dismiss after 5 seconds (errors: 8 seconds). `data-testid="toast-container"`. Toasts slide in from the right (desktop) or bottom (mobile) via a CSS transition.
2. **API error toasts:** all `useMutation` failures that are not handled inline by a specific component emit an error toast via `useToast().add()`. The toast message is derived from the API error response's `message` field if present, otherwise "Something went wrong. Please try again." Unhandled `useQuery` errors also emit toasts after the query has exhausted its retry budget (Tanstack Query default: 3 retries for queries).
3. **Form validation errors:** all form fields show errors inline, below the field, with a red color and `role="alert"` on the error message element. Field-level errors appear on blur or on form submit attempt. They do not appear on first mount or while the field is focused for the first time. Inline errors do not use toast notifications.
4. **Network error — offline detection:** a persistent `OfflineBanner` component monitors `navigator.onLine` via `online`/`offline` window events. When offline, a full-width amber banner at the top of the app reads: "You're offline. Changes may not be saved." The banner dismisses automatically when connectivity is restored and replaces itself with a green "Back online" flash for 2 seconds. `data-testid="offline-banner"`.
5. **Loading skeletons:** every component that fetches data must have a skeleton loading state. Skeleton requirements:
   - Skeletons must match the rough layout of the loaded content (same number of lines, similar widths).
   - All skeletons use `animate-pulse bg-gray-200 dark:bg-gray-700` classes.
   - Skeletons have `aria-hidden="true"` and a visually hidden `<span>` with text "Loading..." for screen readers.
   - Skeletons appear immediately on mount (no delay). If data loads in under 200ms, the skeleton flashes briefly — this is acceptable.
6. **Empty states:** the following components must have defined empty states:
   - Dashboard (S-042 AC 5): no servers. Render illustration + CTA.
   - Document renderer (S-047 AC 8): no data / null / empty object.
   - Selected fields panel (S-048 AC 7): no fields selected.
   - Recent calls table (S-063 AC 4): no calls yet.
   - Playground result (no runs yet): render "Run the tool to see results here" with a muted icon. `data-testid="playground-empty-state"`.
   All empty states include: a centered illustration (SVG icon, not a raster image), a short heading, optional subtext, and an optional CTA. Empty states must not look like error states.
7. **Query error states:** every `useQuery` that can fail must render a dedicated error state (not just a toast). The error state component: `<QueryError message="..." onRetry={refetch} />`. It renders a centered error illustration, a heading "Something went wrong", a message, and a "Try again" button. `data-testid="query-error-state"`. The error state replaces the content area for that component only — other sections of the page remain functional.
8. **Route-level error boundary:** wrap the router's `<Outlet>` in a React `<ErrorBoundary>` component. If an unhandled JS exception propagates to the boundary, render a full-page error state: "An unexpected error occurred" with a "Reload page" button (calls `window.location.reload()`). Log the error to the browser console. `data-testid="route-error-boundary"`.
9. **Retry behavior:** Tanstack Query is configured with `retry: 2` for queries (not mutations). Failed mutations do not retry automatically — the user retries manually. The retry count resets on component remount.
10. **Accessibility for dynamic states:** all dynamically appearing content (toasts, error banners, success messages, loading state transitions) announces itself to screen readers via `aria-live` regions. Use `aria-live="polite"` for non-critical updates (success toasts, loading completion) and `aria-live="assertive"` for error messages that require immediate attention.

### Technical Notes

- The `<QueryError>` component accepts `message?: string` (optional; uses a generic fallback if not provided) and `onRetry?: () => void` (shows the retry button only if provided).
- The `<ToastContainer>` and `useToast()` hook are implemented in this story and imported by all other stories that need toasts. Define the `Toast` type in `src/types/toast.ts`.
- Error boundary: use the `react-error-boundary` package (single dependency addition). Configure `FallbackComponent` to the full-page error state component.
- The offline banner should not interfere with modals. Position it with `z-index` lower than modal overlays.

---

## S-069: Keyboard Shortcuts and Accessibility

**Story ID:** S-069
**Title:** Keyboard shortcuts and accessibility
**Priority:** P1
**Estimated Effort:** 5 points
**Dependencies:** All stories in EPIC-03 and EPIC-04 — this is a cross-cutting audit and remediation story.

### Description

As a user who navigates by keyboard, or uses a screen reader, or relies on high-contrast settings, I want the full platform to be operable without a mouse and to meet WCAG 2.1 AA standards so that I am not excluded from any critical workflow.

### Acceptance Criteria

1. **Tab order:** Tab navigation through the builder flow follows the visual reading order (left-to-right, top-to-bottom). The URL input (S-043) is focused on mount of the `/servers/new` page. Tab order must not get trapped in any section except intentional modal overlays. Verify with keyboard-only testing: a user must be able to complete the full builder flow — URL input → auth → test → field mapping → tool naming → deploy — using Tab, Shift+Tab, Enter, Space, and arrow keys only.
2. **Keyboard shortcuts** (global, active when no input is focused):
   - `n`: navigate to `/servers/new` (equivalent to clicking "New server"). Announce via screen reader as "Navigating to new server form."
   - `Escape`: close the currently open modal or drawer (if any). If no modal, no action.
   - `?`: open a keyboard shortcuts help modal listing all available shortcuts. `data-testid="shortcuts-modal"`.
   - Shortcuts do not activate when focus is inside an input, textarea, or contentEditable element. Check `document.activeElement.tagName` before handling.
3. **Builder-specific shortcuts:**
   - `Cmd+Enter` / `Ctrl+Enter`: advance to the next step in the builder flow (equivalent to clicking "Continue"). Only active when the Continue button would be enabled.
   - `Cmd+Backspace` / `Ctrl+Backspace`: go back one step in the builder flow. Does not trigger browser navigation — calls the in-app back action.
4. **Form keyboard behavior:**
   - In the URL input (S-043): pressing Enter when the URL is valid and the Continue button would be enabled advances to the next step.
   - In the tool name input (S-050): pressing Enter submits/advances if the field is valid.
   - In the delete confirmation input (S-066): pressing Enter submits deletion if the confirm input matches the server name. If it does not match, no action (not even a visual error — the user must type the name correctly first).
   - In the playground parameter form (S-062): pressing Enter in any parameter input triggers "Run tool" if all required fields are filled.
5. **ARIA labels on all interactive elements:**
   - Icon-only buttons (Close, Remove field, drag handle, chevron expand) must have `aria-label` describing their action in context. Example: `<button aria-label="Remove Customer Name field">×</button>`.
   - The method selector dropdown (S-043): `aria-label="HTTP method"`.
   - Auth type segmented control (S-044): `role="radiogroup" aria-label="Authentication type"` with individual options as `role="radio"`.
   - Status badges (S-064): include full status description in `aria-label` (per S-064 AC 10).
6. **Document renderer screen reader support (S-047):**
   - Each renderable value has `role="button"` (since it is clickable) and `aria-label="Select [field name]: [value]"`. After selection, the `aria-label` updates to "Deselect [field name]: [value]".
   - Collapsible sections (`<details>`/`<summary>`) are natively accessible. Verify that the summary reads as "Expand [field name] section" when collapsed.
   - The Selected Fields panel has `role="list"` and each chip has `role="listitem"`.
7. **Color contrast:** all text meets WCAG 2.1 AA contrast ratios: 4.5:1 for normal text, 3:1 for large text (18pt+ or 14pt bold+). This applies to both light and dark mode. Status badge colors (green, yellow, red, gray) must pass contrast in both modes. Verify with an automated tool (Axe or Lighthouse) — no manual exemptions.
8. **Focus indicators:** all focusable elements have a visible focus ring. The ring is `ring-2 ring-brand-500 ring-offset-2` (matching the brand color). The ring is visible in both light and dark mode (use `ring-offset-white dark:ring-offset-gray-900` for contrast). Remove `outline: none` CSS only when a custom ring is confirmed to be present.
9. **Reduced motion:** all animations and transitions in the app respect `prefers-reduced-motion`. When the user has reduced motion enabled:
   - CSS transitions change from `duration-200`/`duration-400` to `duration-0` (instant).
   - The deploy success checkmark animation (S-060) does not animate — the checkmark appears instantly.
   - The toast slide-in animation is disabled — toasts appear instantly.
   Implement via Tailwind's `motion-safe:` and `motion-reduce:` variants on all animated elements.
10. **Automated accessibility test integration:** add `@axe-core/react` to the development dependency list. In `development` mode only, wrap the app in `<React.StrictMode>` and call `axe` on each route navigation, logging violations to the console. This does not block renders — it is a development-time diagnostic. In `production` mode, `axe` is tree-shaken out of the bundle.

### Technical Notes

- Global keyboard shortcut handler: implement as a single `useGlobalShortcuts()` hook mounted at the app root. It attaches one `keydown` listener to `window` and dispatches to registered shortcut handlers. Individual features register shortcuts via a `registerShortcut(keys, handler, description)` API returned by the hook. The shortcuts help modal (`?`) reads from the registered shortcut registry.
- The shortcut `n` for new server: check `document.activeElement` is `document.body` or a non-input element before firing. Never intercept typing.
- `aria-live` regions: place them at the root level so they persist across route changes. Two regions: `aria-live="polite" aria-atomic="true"` for success/info and `aria-live="assertive" aria-atomic="true"` for errors. Announce to them by setting `textContent` directly via a ref (more reliable than state-driven renders for screen reader timing).
- Axe integration: use `react-axe` pattern — `if (process.env.NODE_ENV !== 'production') { import('axe-core').then(axe => axe.default.run().then(results => { if (results.violations.length) console.table(results.violations); })); }` — called in a `useEffect` on route change.
