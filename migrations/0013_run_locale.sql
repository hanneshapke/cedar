-- The user's language and region (a BCP 47 tag such as 'de-CH'). The model replies in the language
-- of each request and formats for this locale; runs from before it count as plain English.
ALTER TABLE runs ADD COLUMN locale TEXT NOT NULL DEFAULT 'en';
