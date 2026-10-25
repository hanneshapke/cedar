-- Android phones that registered for push (the Android app, Settings -> Notifications ->
-- Android push): how the push gets there (`fcm`, Firebase Cloud Messaging, or `unifiedpush`, a
-- distributor such as ntfy), the FCM registration token or the UnifiedPush endpoint URL, and a
-- name to tell them apart in Settings. A token or endpoint reported gone is deleted.
CREATE TABLE android_devices (
    id           TEXT PRIMARY KEY NOT NULL,
    transport    TEXT NOT NULL,
    token        TEXT NOT NULL UNIQUE,
    name         TEXT,
    created_at   INTEGER NOT NULL, -- unix milliseconds
    last_used_at INTEGER
);
