-- ============================================================================
-- La giacenza non è di nostra competenza.
--
-- Il perimetro del progetto è la **generazione della scheda prodotto**: quanti
-- pezzi ci sono a magazzino lo gestisce l'ecommerce, non noi. Tenere una
-- colonna che nessuno aggiorna è peggio che non averla, perché prima o poi
-- qualcuno la legge credendola vera.
-- ============================================================================

ALTER TABLE products DROP COLUMN stock;

-- Di conseguenza cambia anche il significato di una riga di fattura il cui EAN
-- è già a catalogo: non è più "aggiorna la giacenza" ma "non c'è niente da
-- fare". L'enum va rinominato per dire la verità.
ALTER TYPE line_item_action RENAME VALUE 'restock' TO 'skip';
