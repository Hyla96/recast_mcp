/**
 * Clerk appearance configuration.
 *
 * Maps app design tokens (CSS custom properties) to Clerk's appearance API.
 * Returns a plain object matching Clerk's appearance prop shape so the
 * component adapts to the active Tailwind dark-class without importing
 * `@clerk/themes` (which would add a separate package dependency).
 */

import type { SignIn } from '@clerk/clerk-react';
import type { ComponentProps } from 'react';

/** Clerk appearance type inferred from the <SignIn> component props. */
type ClerkAppearance = NonNullable<ComponentProps<typeof SignIn>['appearance']>;

/** Returns a Clerk appearance config for the given theme. */
export function buildClerkAppearance(theme: 'light' | 'dark'): ClerkAppearance {
  const isDark = theme === 'dark';

  return {
    variables: {
      // Typography
      fontFamily: "Inter, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      fontSize: '1rem',
      borderRadius: '0.5rem',

      // Colors — matched to CSS custom properties defined in tokens.css
      colorPrimary: isDark ? '#ffb4a8' : '#a8372c',
      colorTextOnPrimaryBackground: isDark ? '#690005' : '#ffffff',
      colorDanger: isDark ? '#ffb4ab' : '#ba1a1a',

      // Background
      colorBackground: isDark ? '#151914' : '#f0f5ea',
      colorInputBackground: isDark ? '#191d18' : '#ffffff',

      // Text
      colorText: isDark ? '#e0e4da' : '#181d16',
      colorTextSecondary: isDark ? '#c3c8bb' : '#43483f',
      colorInputText: isDark ? '#e0e4da' : '#181d16',

      // Borders / neutrals
      colorNeutral: isDark ? '#8d9288' : '#73786e',
    },
  };
}
