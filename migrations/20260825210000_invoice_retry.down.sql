DROP INDEX IF EXISTS idx_invoices_claimable;
ALTER TABLE invoices DROP COLUMN next_attempt_at;
