-- iPhones that registered for push (the iOS app, Settings -> Notifications -> iPhone push):
-- the device token APNs gave the app, and a name to tell them apart in Settings. A token APNs
-- reports gone (410, or 400 BadDeviceToken) is deleted.
CREATE TABLE apns_devices (
    id           TEXT PRIMARY KEY NOT NULL,
    token        TEXT NOT NULL UNIQUE,
    name         TEXT,
    created_at   INTEGER NOT NULL, -- unix milliseconds
    last_used_at INTEGER
);
