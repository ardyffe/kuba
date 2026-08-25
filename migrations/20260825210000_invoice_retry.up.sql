-- ============================================================================
-- Backoff dei tentativi.
--
-- Quando la lavorazione di una fattura fallisce per una causa passeggera (il
-- database non risponde, l'API è momentaneamente giù) ha senso riprovare — ma
-- non subito, e non all'infinito. Serve un momento a partire dal quale la
-- fattura torna prendibile: fino ad allora resta 'pending' ma invisibile al
-- worker.
--
-- NULL significa "prendibile adesso", che è il caso di ogni nuovo caricamento.
-- ============================================================================

ALTER TABLE invoices ADD COLUMN next_attempt_at TIMESTAMPTZ;

-- L'indice serve alla query di claim, che gira in continuazione: senza,
-- ogni giro sarebbe una scansione completa della tabella.
CREATE INDEX idx_invoices_claimable ON invoices (next_attempt_at, uploaded_at)
    WHERE status = 'pending';
