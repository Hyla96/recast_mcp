/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_CLERK_PUBLISHABLE_KEY: string | undefined;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
