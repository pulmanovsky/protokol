-- Small key/value store for app-level flags (e.g. consent to use cloud models)
CREATE TABLE IF NOT EXISTS app_flags (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
