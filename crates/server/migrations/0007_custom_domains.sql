-- Workspace custom play domains. A workspace owner attaches a domain they
-- control (e.g. `play.acme.com`); the workspace's share links are then also
-- served at `https://<slug>.<custom_play_domain>/` with on-demand TLS, in
-- addition to the platform's own `<slug>.<play_domain>` host.
--
-- Stored lowercase (normalized by the API before write). UNIQUE so a TLS-ask /
-- host lookup on the domain suffix resolves at most one workspace, and so two
-- workspaces can never claim the same domain (409 domain_taken on conflict).
ALTER TABLE workspaces ADD COLUMN custom_play_domain TEXT UNIQUE;
