# Upgrading config/app.toml to schema version 2

`config/app.toml` is the user's own copy and holds their changes.
`config/app.defaults.toml` holds the version 2 defaults; do not edit it.

1. Set `schema_version = 2`.
2. `server.timeout` (seconds) is now `server.timeout_ms`
   (milliseconds): replace the old key with the new one, converting
   the user's value. Do not keep the old key.
3. Add every section and key of the defaults that `app.toml` lacks,
   with its default value.
4. Keep every value the user set, even where it differs from the
   defaults, and keep their comments and their own sections.
