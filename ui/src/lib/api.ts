// Barrel that re-exports both API halves for existing desktop-chrome imports.
//
// NOTE: importing from `$lib/api` pulls in `./api.tauri` and therefore a
// `@tauri-apps` dependency. Tauri-free consumers (the `/test` route, and any
// future cloud/share workbench) must import from `$lib/api.http` directly so
// their chunk stays Tauri-free.
export * from './api.http';
export * from './api.tauri';
