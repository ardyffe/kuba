ALTER TYPE line_item_action RENAME VALUE 'skip' TO 'restock';

ALTER TABLE products ADD COLUMN stock INTEGER NOT NULL DEFAULT 0;
