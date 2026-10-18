-- Server-wide settings an admin changes through the API (`PUT /v1/admin/settings`), one JSON value
-- per key; today only `max_image_side`. A missing key means the built-in default.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL -- JSON
);
