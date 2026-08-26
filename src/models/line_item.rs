//! Le righe di una fattura, così come finiscono nella tabella
//! `invoice_line_items`.

use rust_decimal::Decimal;
use serde::Serialize;
use uuid::Uuid;

/// Che cosa rappresenta una riga.
///
/// Non tutto ciò che è stampato nella tabella di una fattura è un prodotto:
/// `invoice1` ha una riga `Standard International 0,00 €` che è spedizione.
/// Senza questa distinzione l'agente proverebbe a costruirci una scheda profumo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "line_item_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LineItemKind {
    Product,
    Shipping,
    Discount,
    Unknown,
}

impl LineItemKind {
    /// Converte la stringa che arriva dal modello.
    ///
    /// Lo schema JSON vincola già i valori possibili, quindi il ramo `_` non
    /// dovrebbe mai scattare. Lo teniamo comunque: fidarsi di un vincolo
    /// imposto da un sistema remoto e non avere un piano B è come non avere
    /// vincolo. Nel dubbio `Unknown`, che manda la riga alla revisione umana
    /// invece di farne un prodotto sbagliato.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "product" => Self::Product,
            "shipping" => Self::Shipping,
            "discount" => Self::Discount,
            _ => Self::Unknown,
        }
    }
}

/// Cosa deve farne l'agente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "line_item_action", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LineItemAction {
    /// L'EAN è già a catalogo: per noi non c'è niente da fare.
    Skip,
    /// Prodotto nuovo, va creata la scheda.
    Create,
    /// Dati insufficienti (EAN mancante, riga ambigua): serve un umano.
    NeedsReview,
}

/// A che punto è la lavorazione della singola riga.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "line_item_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LineItemStatus {
    Pending,
    Matched,
    Enriched,
    Done,
    Failed,
}

/// Una riga di fattura come esce dall'API.
#[derive(Debug, Serialize)]
pub struct InvoiceLineItem {
    pub id: Uuid,
    pub line_no: i32,
    /// Il testo com'è stampato sulla fattura. È la rete di sicurezza: quando
    /// l'estrazione sbaglia, questo è il campo che permette di accorgersene
    /// senza riaprire il PDF.
    pub raw_text: String,
    pub description: Option<String>,
    pub ean: Option<String>,
    pub supplier_sku: Option<String>,
    pub quantity: Option<i32>,
    pub unit_price: Option<Decimal>,
    pub amount: Option<Decimal>,
    pub kind: LineItemKind,
    pub action: Option<LineItemAction>,
    pub status: LineItemStatus,
    pub matched_product_id: Option<Uuid>,
    pub error_message: Option<String>,
}
