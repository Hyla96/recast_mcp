# Design System: Editorial API Interface
## Agent-Optimized Reference v2.0

---

## HARD RULES — Never violate

These rules are absolute. Violations break the design language.

```
RULE 001: NO STRUCTURAL BORDERS
├─ PROHIBITED: 1px solid borders for sectioning content
├─ PROHIBITED: Divider lines between list items  
├─ PROHIBITED: Border-bottom on section headers
└─ EXCEPTION: Ghost borders only (outline-variant at 15% opacity)

RULE 002: NO PURE BLACK
├─ PROHIBITED: #000000 for any text or UI element
├─ REQUIRED: Use on-surface (#181d16) for all dark text
└─ REQUIRED: Use inverse-surface (#2d322b) for dark backgrounds

RULE 003: NO CROWDED EDGES
├─ PROHIBITED: Content touching container edges
├─ REQUIRED: Minimum spacing-8 (0.5rem) internal padding
└─ REQUIRED: Let surface background (#f6fbf0) show through gaps

RULE 004: NO DROP SHADOWS
├─ PROHIBITED: Standard box-shadow for depth
├─ REQUIRED: Achieve depth via tonal layering (see Surface Hierarchy)
└─ EXCEPTION: Ambient shadow for floating elements only
```

---

## TOKEN REFERENCE — Quick lookup

### Color tokens

| Token | Hex | Role | Usage |
|-------|-----|------|-------|
| `primary` | #a8372c | Brand / High-intent | Primary buttons, critical actions |
| `primary-container` | #ed6a5a | Brand accent | Gradient endpoints, hover states |
| `secondary` | #15686d | Connectivity | Data viz, status indicators, links |
| `secondary-container` | #a6eff4 | Secondary accent | Highlights, selected states |
| `tertiary` | #616036 | Soft alerts | Code strings, new feature badges |
| `tertiary-container` | #fcf7c1 | Tertiary accent | Soft highlight backgrounds |
| `surface` | #f6fbf0 | Base canvas | Page background |
| `surface-container-lowest` | #ffffff | Maximum lift | Interactive cards, modals |
| `surface-container-low` | #f0f5ea | Sectioning | Content groups, input backgrounds |
| `surface-container` | #eaefe4 | Mid-level | Secondary sections |
| `surface-container-high` | #e5eadf | Recessed | Sidebars, utility panels |
| `surface-container-highest` | #dfe4d9 | Hover state | List item hover |
| `surface-variant` | #dfe4d9 | Muted surface | Disabled backgrounds |
| `inverse-surface` | #2d322b | Dark mode | Code blocks, dark sections |
| `on-surface` | #181d16 | Primary text | Headings, body copy |
| `on-surface-variant` | #43483f | Secondary text | Labels, captions |
| `inverse-on-surface` | #edf2e7 | Light text | Text on dark backgrounds |
| `outline` | #73786e | Borders | Ghost button text, subtle borders |
| `outline-variant` | #c3c8bb | Faint borders | Ghost borders (use at 15% opacity) |
| `on-primary` | #ffffff | Contrast text | Text on primary buttons |
| `error` | #ba1a1a | Error state | Validation errors |
| `error-container` | #ffdad6 | Error background | Error message backgrounds |

### Surface hierarchy (nesting order)

```
LAYER 0: surface (#f6fbf0)              ← Page canvas
  └─ LAYER 1: surface-container-low (#f0f5ea)    ← Section groups
       └─ LAYER 2: surface-container-lowest (#ffffff) ← Interactive cards
```

Always nest in this order. Never place `surface-container-lowest` directly on `surface`.

### Spacing tokens

| Token | Value | Category | Usage |
|-------|-------|----------|-------|
| `spacing-1` | 0.0625rem (1px) | Micro | Hairline gaps |
| `spacing-2` | 0.125rem (2px) | Micro | Icon gaps |
| `spacing-3` | 0.1875rem (3px) | Micro | Tight padding |
| `spacing-4` | 0.25rem (4px) | Micro | Button internal padding |
| `spacing-6` | 0.375rem (6px) | Micro | Chip padding |
| `spacing-8` | 0.5rem (8px) | Macro | Component gaps |
| `spacing-10` | 0.625rem (10px) | Macro | Small margins |
| `spacing-12` | 0.75rem (12px) | Macro | Section separation |
| `spacing-16` | 1rem (16px) | Macro | Major section margins |
| `spacing-20` | 1.25rem (20px) | Macro | Large gaps |
| `spacing-24` | 1.5rem (24px) | Gutter | Asymmetric layouts |
| `spacing-32` | 2rem (32px) | Gutter | Section headers bottom margin |

### Border radius tokens

| Token | Value | Usage |
|-------|-------|-------|
| `radius-none` | 0 | Sharp corners (code blocks) |
| `radius-xs` | 0.125rem (2px) | Tags, micro badges |
| `radius-sm` | 0.25rem (4px) | Chips, small buttons |
| `radius-md` | 0.5rem (8px) | Buttons, inputs, cards |
| `radius-lg` | 0.75rem (12px) | Modals, large cards |
| `radius-xl` | 1rem (16px) | Hero sections |
| `radius-full` | 9999px | Circular avatars, pills |

### Typography tokens

| Token | Size | Weight | Line-height | Letter-spacing | Usage |
|-------|------|--------|-------------|----------------|-------|
| `display-lg` | 3.5rem (56px) | 400 | 1.15 | -0.02em | Hero metrics |
| `display-md` | 2.75rem (44px) | 400 | 1.15 | -0.02em | Dashboard numbers |
| `display-sm` | 2.25rem (36px) | 400 | 1.2 | -0.02em | Large callouts |
| `headline-lg` | 2rem (32px) | 400 | 1.25 | -0.01em | Page titles |
| `headline-md` | 1.75rem (28px) | 400 | 1.3 | 0 | Section titles |
| `headline-sm` | 1.5rem (24px) | 400 | 1.35 | 0 | Card headers |
| `title-lg` | 1.375rem (22px) | 500 | 1.4 | 0 | Subsection headers |
| `title-md` | 1rem (16px) | 500 | 1.5 | 0.01em | List headers |
| `title-sm` | 0.875rem (14px) | 500 | 1.5 | 0.01em | Small headers |
| `body-lg` | 1rem (16px) | 400 | 1.6 | 0 | Primary content |
| `body-md` | 0.875rem (14px) | 400 | 1.6 | 0 | Secondary content |
| `body-sm` | 0.75rem (12px) | 400 | 1.5 | 0 | Captions |
| `label-lg` | 0.875rem (14px) | 500 | 1.4 | 0.01em | Form labels |
| `label-md` | 0.75rem (12px) | 500 | 1.4 | 0.02em | Buttons, tags |
| `label-sm` | 0.6875rem (11px) | 500 | 1.3 | 0.05em | Overlines (UPPERCASE) |

**Font stack:** `'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif`

### Elevation tokens

| Token | Value | Usage |
|-------|-------|-------|
| `elevation-0` | none | Flat elements |
| `elevation-1` | Tonal layering only | Cards on sections |
| `elevation-2` | `0 20px 40px rgba(24, 29, 22, 0.06)` | Floating elements |
| `elevation-3` | `0 24px 48px rgba(24, 29, 22, 0.08)` | Modals |

### Z-index scale

| Token | Value | Usage |
|-------|-------|-------|
| `z-base` | 0 | Default content |
| `z-dropdown` | 100 | Dropdowns, tooltips |
| `z-sticky` | 200 | Sticky headers |
| `z-fixed` | 300 | Fixed navigation |
| `z-modal-backdrop` | 400 | Modal overlay |
| `z-modal` | 500 | Modal content |
| `z-toast` | 600 | Toast notifications |

### Transition tokens

| Token | Value | Usage |
|-------|-------|-------|
| `duration-fast` | 100ms | Micro interactions |
| `duration-normal` | 200ms | Standard transitions |
| `duration-slow` | 300ms | Complex animations |
| `easing-standard` | cubic-bezier(0.4, 0, 0.2, 1) | Default easing |
| `easing-decelerate` | cubic-bezier(0, 0, 0.2, 1) | Enter animations |
| `easing-accelerate` | cubic-bezier(0.4, 0, 1, 1) | Exit animations |

---

## DECISION TREES — Pattern selection

### Separating content blocks

```
IF adjacent sections need visual separation:
  ├─ FIRST: Add spacing-16 between them
  │   └─ IF still unclear visually:
  │       ├─ THEN: Shift background (surface → surface-container-low)
  │       └─ IF accessibility requires explicit boundary:
  │           └─ LAST RESORT: Ghost border (outline-variant at 15% opacity)
  └─ NEVER: 1px solid border
```

### Button hierarchy selection

```
IF action type is:
  ├─ PRIMARY (submit, save, confirm, CTA):
  │   └─ USE: Gradient primary→primary-container, on-primary text, radius-md
  │
  ├─ SECONDARY (cancel, back, alternative):
  │   └─ USE: Ghost style (transparent bg, outline text, surface-variant hover)
  │
  ├─ TERTIARY (learn more, details, minor actions):
  │   └─ USE: Text only, no container, underline on hover
  │
  └─ DESTRUCTIVE (delete, remove):
      └─ USE: Ghost style with error color text
```

### Input field states

```
IF input state is:
  ├─ DEFAULT:
  │   ├─ background: surface-container-low
  │   ├─ border: none
  │   └─ label: on-surface-variant, floated above
  │
  ├─ FOCUSED:
  │   ├─ background: surface-container-low
  │   ├─ border-bottom: 2px solid secondary (#15686d)
  │   └─ label: secondary color
  │
  ├─ ERROR:
  │   ├─ background: error-container (10% opacity)
  │   ├─ border-bottom: 2px solid error
  │   └─ helper text: error color
  │
  └─ DISABLED:
      ├─ background: surface-variant
      ├─ opacity: 0.6
      └─ cursor: not-allowed
```

### Card depth selection

```
IF card context is:
  ├─ INTERACTIVE (clickable, API endpoint, list item):
  │   ├─ background: surface-container-lowest (#ffffff)
  │   ├─ parent: surface-container-low (#f0f5ea)
  │   └─ hover: surface-container-highest background shift
  │
  ├─ INFORMATIONAL (static display, metrics):
  │   ├─ background: surface-container-low
  │   └─ parent: surface
  │
  └─ FLOATING (modal, popover, dropdown):
      ├─ background: surface-container-lowest
      ├─ shadow: elevation-2
      └─ backdrop: blur(12px) if overlaying content
```

### Section header treatment

```
IF element is section header:
  ├─ USE: headline-sm (1.5rem)
  ├─ MARGIN: spacing-32 (2rem) bottom
  ├─ BORDER: none (never underline)
  └─ OPTIONAL: label-sm overline above in on-surface-variant, uppercase
```

---

## CODE TEMPLATES — Copy-paste ready

### Primary button

```css
.btn-primary {
  background: linear-gradient(135deg, #a8372c 0%, #ed6a5a 100%);
  color: #ffffff;
  border: none;
  border-radius: 0.5rem;
  padding: 0.75rem 1.5rem;
  font-family: 'Inter', sans-serif;
  font-size: 0.75rem;
  font-weight: 500;
  letter-spacing: 0.02em;
  cursor: pointer;
  transition: opacity 200ms cubic-bezier(0.4, 0, 0.2, 1);
}

.btn-primary:hover {
  opacity: 0.9;
}

.btn-primary:active {
  transform: scale(0.98);
}
```

### Secondary button (ghost)

```css
.btn-secondary {
  background: transparent;
  color: #73786e; /* outline */
  border: none;
  border-radius: 0.5rem;
  padding: 0.75rem 1.5rem;
  font-family: 'Inter', sans-serif;
  font-size: 0.75rem;
  font-weight: 500;
  letter-spacing: 0.02em;
  cursor: pointer;
  transition: background 200ms cubic-bezier(0.4, 0, 0.2, 1);
}

.btn-secondary:hover {
  background: #dfe4d9; /* surface-variant */
}
```

### Card (tonal layering)

```css
/* Parent container */
.card-group {
  background: #f0f5ea; /* surface-container-low */
  padding: 1rem;
  border-radius: 0.75rem;
}

/* Interactive card inside */
.card {
  background: #ffffff; /* surface-container-lowest */
  border: none; /* NO BORDERS */
  border-radius: 0.5rem;
  padding: 1.25rem;
  transition: background 200ms cubic-bezier(0.4, 0, 0.2, 1);
}

.card:hover {
  background: #dfe4d9; /* surface-container-highest */
}
```

### Floating element (modal/popover)

```css
.modal {
  background: rgba(255, 255, 255, 0.95); /* surface-container-lowest */
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  border-radius: 0.75rem;
  box-shadow: 0 20px 40px rgba(24, 29, 22, 0.06);
  border: none;
}
```

### Input field

```css
.input-field {
  background: #f0f5ea; /* surface-container-low */
  border: none;
  border-bottom: 2px solid transparent;
  border-radius: 0.5rem 0.5rem 0 0;
  padding: 1rem;
  font-family: 'Inter', sans-serif;
  font-size: 0.875rem;
  color: #181d16; /* on-surface */
  transition: border-color 200ms cubic-bezier(0.4, 0, 0.2, 1);
}

.input-field:focus {
  outline: none;
  border-bottom-color: #15686d; /* secondary */
}

.input-label {
  font-size: 0.75rem;
  font-weight: 500;
  color: #43483f; /* on-surface-variant */
  margin-bottom: 0.25rem;
}
```

### Code block

```css
.code-block {
  background: #2d322b; /* inverse-surface */
  color: #edf2e7; /* inverse-on-surface */
  border: none;
  border-radius: 0.5rem;
  padding: 1rem;
  font-family: 'JetBrains Mono', 'Fira Code', monospace;
  font-size: 0.875rem;
  line-height: 1.6;
  overflow-x: auto;
}

.code-block .string {
  color: #fcf7c1; /* tertiary-container */
}

.code-block .method {
  color: #a6eff4; /* secondary-container */
}

.code-block .keyword {
  color: #ed6a5a; /* primary-container */
}
```

### List item (no dividers)

```css
.list-item {
  padding: 1rem;
  border: none; /* NO DIVIDERS */
  transition: background 200ms cubic-bezier(0.4, 0, 0.2, 1);
}

.list-item:hover {
  background: #dfe4d9; /* surface-container-highest */
}

/* Separation via spacing, not lines */
.list-item + .list-item {
  margin-top: 0.5rem; /* spacing-8 */
}
```

### Section header with overline

```css
.section-header {
  margin-bottom: 2rem; /* spacing-32 */
}

.section-overline {
  font-size: 0.6875rem;
  font-weight: 500;
  letter-spacing: 0.05em;
  text-transform: uppercase;
  color: #43483f; /* on-surface-variant */
  margin-bottom: 0.25rem;
}

.section-title {
  font-size: 1.5rem; /* headline-sm */
  font-weight: 400;
  letter-spacing: 0;
  color: #181d16; /* on-surface */
  /* NO border-bottom */
}
```

### Ghost border (accessibility fallback)

```css
/* ONLY when required for accessibility */
.ghost-bordered {
  border: 1px solid rgba(195, 200, 187, 0.15); /* outline-variant at 15% */
}
```

### Glass navigation

```css
.nav-glass {
  position: fixed;
  background: rgba(255, 255, 255, 0.8);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  border: none;
  z-index: 300; /* z-fixed */
}
```

---

## COMPONENT SPECIFICATIONS

### Buttons

| Variant | Background | Text | Border | Radius | Padding |
|---------|------------|------|--------|--------|---------|
| Primary | gradient(135deg, primary → primary-container) | on-primary | none | radius-md | 0.75rem 1.5rem |
| Secondary | transparent → surface-variant (hover) | outline | none | radius-md | 0.75rem 1.5rem |
| Tertiary | none | outline | none (underline hover) | 0 | 0.5rem 0 |
| Destructive | transparent → error-container (hover) | error | none | radius-md | 0.75rem 1.5rem |

### Form elements

| Element | Background | Border | Focus state |
|---------|------------|--------|-------------|
| Text input | surface-container-low | none, 2px bottom transparent | 2px bottom secondary |
| Textarea | surface-container-low | none, 2px bottom transparent | 2px bottom secondary |
| Select | surface-container-low | none, 2px bottom transparent | 2px bottom secondary |
| Checkbox | surface-container-low | 2px outline | secondary fill when checked |
| Radio | surface-container-low | 2px outline | secondary fill when selected |
| Toggle | outline (track) | none | secondary (track active) |

### Cards

| Type | Background | Shadow | Border | Hover |
|------|------------|--------|--------|-------|
| Interactive | surface-container-lowest | none (tonal depth) | none | background → surface-container-highest |
| Static | surface-container-low | none | none | none |
| Floating | surface-container-lowest | elevation-2 | none | none |

### Status indicators

| Status | Background | Text | Icon |
|--------|------------|------|------|
| Success | secondary-container (15%) | secondary | ✓ secondary |
| Warning | tertiary-container | tertiary | ⚠ tertiary |
| Error | error-container (15%) | error | ✕ error |
| Info | secondary-container (10%) | secondary | ℹ secondary |

---

## ACCESSIBILITY REQUIREMENTS

### Focus states

```css
/* All interactive elements must have visible focus */
:focus-visible {
  outline: 2px solid #15686d; /* secondary */
  outline-offset: 2px;
}

/* Remove default focus ring */
:focus:not(:focus-visible) {
  outline: none;
}
```

### Color contrast minimums

| Context | Minimum ratio | Compliant pairs |
|---------|---------------|-----------------|
| Normal text | 4.5:1 | on-surface on surface ✓ |
| Large text (≥18px) | 3:1 | on-surface-variant on surface ✓ |
| UI components | 3:1 | secondary on surface ✓ |
| Disabled | No minimum | Use opacity 0.6 |

### Motion preferences

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

---

## DARK MODE TOKENS

If implementing dark mode, map tokens as follows:

| Light token | Dark value |
|-------------|------------|
| surface | #121714 |
| surface-container-low | #1b1f1a |
| surface-container-lowest | #0d110e |
| surface-container-high | #252a24 |
| on-surface | #e0e4da |
| on-surface-variant | #c3c8bb |
| primary | #ffb4a8 |
| primary-container | #8e1d12 |
| secondary | #a6eff4 |
| secondary-container | #004f54 |

---

## VALIDATION CHECKLIST — Self-check before output

Run through this checklist before finalizing any UI output:

```
□ No `border: 1px solid` for section boundaries
  └─ Separation achieved via spacing or background shift only

□ No #000000 anywhere
  └─ All dark text uses on-surface (#181d16)

□ Tonal layering correct
  └─ Cards (lowest) sit on sections (low), sections sit on surface

□ Button hierarchy correct
  └─ Primary = gradient, Secondary = ghost, Tertiary = text only

□ Spacing uses tokens only
  └─ No arbitrary pixel values (use spacing-2 through spacing-32)

□ Section headers have no underlines
  └─ Use spacing-32 bottom margin instead

□ Input fields have no visible borders
  └─ Bottom accent on focus only (2px secondary)

□ Focus states present
  └─ All interactive elements have :focus-visible outline

□ No pure white backgrounds on page level
  └─ Use surface (#f6fbf0) as base canvas

□ Floating elements use ambient shadow
  └─ 0 20px 40px rgba(24, 29, 22, 0.06), never solid black

□ Code blocks use inverse-surface
  └─ Background #2d322b, text #edf2e7

□ Typography uses Inter only
  └─ Correct weights: 400 regular, 500 medium

□ Transitions use standard easing
  └─ cubic-bezier(0.4, 0, 0.2, 1), 200ms default
```

---

## PHILOSOPHY — Context (not rules)

> **Creative North Star: "The Digital Curator"**
>
> Most API management platforms feel like spreadsheets—rigid, cold, and exhausting. This design system rejects the "SaaS-in-a-box" aesthetic in favor of **High-End Editorial Precision**. We treat technical data as curated content.
>
> The system breaks the "template" look by favoring **Tonal Layering** over structural lines. We do not cage content in borders; we allow it to sit on "pedestals" of shifting light and color.

### Design principles

1. **White space is a border.** If you feel the urge to draw a line, add 16px of padding instead.

2. **Depth through tone, not shadow.** Nest surfaces to create visual hierarchy without heavy drop shadows.

3. **Warm professionalism.** The Soft Linen palette reduces eye strain while maintaining sophistication.

4. **Asymmetric layouts welcome.** Place large display text with wide gutters (spacing-24) before content begins.

5. **Let the background breathe.** The #f6fbf0 surface is a luxury—show it through gaps in your layout.

---

## CHANGELOG

| Version | Date | Changes |
|---------|------|---------|
| 2.0 | 2025 | Agent-optimized structure, decision trees, code templates, validation checklist |
| 1.0 | — | Original editorial specification |
