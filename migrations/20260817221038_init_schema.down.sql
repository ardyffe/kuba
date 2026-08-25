-- L'ordine è l'inverso della up: prima le tabelle che referenziano, poi le altre.
DROP TABLE IF EXISTS invoice_line_items;
DROP TABLE IF EXISTS product_images;
DROP TABLE IF EXISTS products;
DROP TABLE IF EXISTS invoices;

DROP TYPE IF EXISTS product_status;
DROP TYPE IF EXISTS line_item_status;
DROP TYPE IF EXISTS line_item_action;
DROP TYPE IF EXISTS line_item_kind;
DROP TYPE IF EXISTS invoice_status;
