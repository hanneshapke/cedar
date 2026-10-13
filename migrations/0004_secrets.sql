-- Integration credentials saved through the admin API (a Brave Search key, a Google refresh token),
-- encrypted with CEDAR_SECRET_KEY. `meta` is non-secret JSON shown to admins (last 4 characters,
-- the Google account).
CREATE TABLE secrets (
    name       TEXT PRIMARY KEY NOT NULL,
    nonce      BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    meta       TEXT NOT NULL,
    updated_at INTEGER NOT NULL -- unix milliseconds
);
