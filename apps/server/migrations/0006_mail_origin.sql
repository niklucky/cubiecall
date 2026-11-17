-- Keep the browser origin across delivery retries and worker restarts.
ALTER TABLE mail_requests ADD COLUMN origin TEXT;
-- These rows predate the domain cutover. Custom deployments validate this
-- origin against their configuration and fall back to their own public URL.
UPDATE mail_requests SET origin='https://quipcall.com';
