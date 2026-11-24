-- A sign-in email carries a 6-digit code next to its link. The code only works
-- with the challenge the asking browser or app was given, a few times.
ALTER TABLE mail_requests ADD COLUMN challenge TEXT;
-- Asked from an app: its email has the code alone, since a link would sign in
-- the browser.
ALTER TABLE mail_requests ADD COLUMN app INTEGER NOT NULL DEFAULT 0;
ALTER TABLE magic_links ADD COLUMN challenge TEXT;
ALTER TABLE magic_links ADD COLUMN code_hash TEXT;
ALTER TABLE magic_links ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
CREATE INDEX magic_links_challenge ON magic_links(challenge);
