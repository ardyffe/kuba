-- ============================================================================
-- Schema iniziale: fatture -> righe fattura -> prodotti
--
-- Nota: da Postgres 13 gen_random_uuid() è una funzione built-in del core,
-- quindi non serve più l'estensione "uuid-ossp" (legacy).
-- ============================================================================

-- ---------------------------------------------------------------------------
-- Enum: stati e classificazioni.
-- Usiamo enum SQL invece di TEXT perché il DB rifiuta i valori non previsti e
-- perché li mappiamo su enum Rust, ottenendo il match esaustivo dal compilatore.
-- ---------------------------------------------------------------------------
CREATE TYPE invoice_status AS ENUM ('pending', 'in_progress', 'succeeded', 'failed');

-- Non tutte le righe di una fattura sono prodotti: es. "Standard International 0,00 EUR"
-- è una riga di spedizione e non deve generare una scheda prodotto.
CREATE TYPE line_item_kind AS ENUM ('product', 'shipping', 'discount', 'unknown');

-- Cosa deve farne l'agente: il prodotto esiste già (aggiorna giacenza),
-- va creato da zero, oppure i dati non bastano e serve un umano.
CREATE TYPE line_item_action AS ENUM ('restock', 'create', 'needs_review');

CREATE TYPE line_item_status AS ENUM ('pending', 'matched', 'enriched', 'done', 'failed');

CREATE TYPE product_status AS ENUM ('draft', 'published', 'deleted');

-- ---------------------------------------------------------------------------
-- products: il catalogo. È anche la staging area dove l'agente scrive le
-- schede in stato 'draft', prima della review umana.
-- ---------------------------------------------------------------------------
CREATE TABLE products (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Identificatori commerciali. L'EAN è la chiave di deduplica: se un EAN
    -- è già a catalogo la fattura deve solo aggiornare la giacenza.
    ean              TEXT UNIQUE,
    sku              TEXT,

    -- Contenuto della scheda (i campi della UI del cliente).
    title            TEXT NOT NULL,
    description      TEXT,
    summary          TEXT,
    meta_title       TEXT,
    meta_description TEXT,
    slug             TEXT,
    brand            TEXT,
    locale           TEXT NOT NULL DEFAULT 'it-IT',

    -- Attributi variabili (note di testa/cuore/fondo, famiglia olfattiva,
    -- genere, ml, durata, scia, "a chi piace questo profumo piace anche"...).
    -- JSONB perché l'insieme delle feature cambia nel tempo e per categoria.
    attributes       JSONB NOT NULL DEFAULT '{}',
    categories       TEXT[] NOT NULL DEFAULT '{}',

    -- Prezzi in NUMERIC, mai in floating point: 0.1 + 0.2 != 0.3 in binario.
    unit_cost        NUMERIC(12, 2),
    price            NUMERIC(12, 2),
    stock            INTEGER NOT NULL DEFAULT 0,

    status           product_status NOT NULL DEFAULT 'draft',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE product_images (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    product_id   UUID NOT NULL REFERENCES products(id) ON DELETE CASCADE,
    source_url   TEXT NOT NULL,
    storage_path TEXT,
    position     INTEGER NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- invoices: il PDF caricato. È anche la coda di lavoro dell'agente:
-- il worker cerca le fatture in stato 'pending'.
-- ---------------------------------------------------------------------------
CREATE TABLE invoices (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Il file vive sul volume; qui teniamo solo il puntatore e i metadati.
    original_filename TEXT NOT NULL,
    storage_path      TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    size_bytes        BIGINT NOT NULL,
    -- Impronta del contenuto: impedisce di caricare due volte la stessa fattura.
    sha256            TEXT NOT NULL UNIQUE,

    -- Dati di testata, compilati dall'agente in fase di estrazione.
    supplier_name     TEXT,
    invoice_number    TEXT,
    invoice_date      DATE,
    currency          TEXT,
    total_amount      NUMERIC(12, 2),

    -- Macchina a stati vista dal frontend.
    status            invoice_status NOT NULL DEFAULT 'pending',
    error_message     TEXT,
    attempts          INTEGER NOT NULL DEFAULT 0,

    uploaded_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at        TIMESTAMPTZ,
    finished_at       TIMESTAMPTZ
);

-- ---------------------------------------------------------------------------
-- invoice_line_items: una riga della fattura. Ha uno stato proprio, così una
-- riga problematica non fa fallire l'intera fattura.
-- ---------------------------------------------------------------------------
CREATE TABLE invoice_line_items (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id         UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,

    line_no            INTEGER NOT NULL,
    -- Il testo grezzo estratto dal PDF: serve per il debug quando l'agente sbaglia.
    raw_text           TEXT NOT NULL,

    description        TEXT,
    ean                TEXT,
    supplier_sku       TEXT,
    quantity           INTEGER,
    unit_price         NUMERIC(12, 2),
    amount             NUMERIC(12, 2),

    kind               line_item_kind NOT NULL DEFAULT 'unknown',
    action             line_item_action,
    status             line_item_status NOT NULL DEFAULT 'pending',

    -- Il prodotto a cui la riga è stata collegata (esistente o appena creato).
    matched_product_id UUID REFERENCES products(id) ON DELETE SET NULL,
    error_message      TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Rende l'estrazione idempotente: un retry non duplica le righe.
    UNIQUE (invoice_id, line_no)
);

-- Da quale riga di fattura nasce una scheda prodotto (tracciabilità).
-- Aggiunto dopo la CREATE TABLE perché le due tabelle si referenziano a vicenda.
ALTER TABLE products
    ADD COLUMN source_line_item_id UUID REFERENCES invoice_line_items(id) ON DELETE SET NULL;

-- ---------------------------------------------------------------------------
-- Indici: solo quelli che servono alle query che scriveremo davvero.
-- ---------------------------------------------------------------------------
CREATE INDEX idx_invoices_status ON invoices(status);
CREATE INDEX idx_invoice_line_items_invoice_id ON invoice_line_items(invoice_id);
CREATE INDEX idx_invoice_line_items_status ON invoice_line_items(status);
CREATE INDEX idx_invoice_line_items_ean ON invoice_line_items(ean);
CREATE INDEX idx_products_status ON products(status);
CREATE INDEX idx_product_images_product_id ON product_images(product_id);
