/**
 * Il client verso il backend Rust.
 *
 * I tipi qui dentro rispecchiano le struct che il backend serializza. Non c'è
 * generazione automatica: sono scritti a mano e vanno tenuti allineati. Con una
 * decina di campi è il compromesso giusto — un generatore di tipi da OpenAPI
 * sarebbe più infrastruttura di quanta ne serva oggi.
 */

const BASE = import.meta.env.VITE_API_URL ?? 'http://127.0.0.1:3000';

export type InvoiceStatus = 'pending' | 'in_progress' | 'succeeded' | 'failed';
export type LineItemKind = 'product' | 'shipping' | 'discount' | 'unknown';
export type LineItemAction = 'skip' | 'create' | 'needs_review';
export type LineItemStatus = 'pending' | 'matched' | 'enriched' | 'done' | 'failed';
export type ProductStatus = 'draft' | 'published' | 'deleted';

export interface Invoice {
	id: string;
	original_filename: string;
	size_bytes: number;
	sha256: string;
	supplier_name: string | null;
	invoice_number: string | null;
	invoice_date: string | null;
	currency: string | null;
	total_amount: string | null;
	status: InvoiceStatus;
	error_message: string | null;
	uploaded_at: string;
}

export interface InvoiceLineItem {
	id: string;
	line_no: number;
	raw_text: string;
	description: string | null;
	ean: string | null;
	supplier_sku: string | null;
	quantity: number | null;
	unit_price: string | null;
	amount: string | null;
	kind: LineItemKind;
	action: LineItemAction | null;
	status: LineItemStatus;
	matched_product_id: string | null;
	error_message: string | null;
}

export type InvoiceDetail = Invoice & { lines: InvoiceLineItem[] };

export interface ProductSummary {
	id: string;
	ean: string | null;
	sku: string | null;
	title: string;
	brand: string | null;
	price: string | null;
	status: ProductStatus;
	updated_at: string;
}

export interface Product extends ProductSummary {
	description: string | null;
	summary: string | null;
	meta_title: string | null;
	meta_description: string | null;
	slug: string | null;
	locale: string;
	attributes: Record<string, unknown>;
	categories: string[];
	unit_cost: string | null;
	created_at: string;
}

/** L'errore così come lo restituisce il backend. */
export class ApiError extends Error {
	constructor(
		public status: number,
		public code: string,
		message: string
	) {
		super(message);
	}
}

/**
 * Wrapper unico su fetch.
 *
 * Tutta l'API risponde con lo stesso formato d'errore `{error:{code,message}}`,
 * quindi la traduzione in eccezione si scrive una volta sola. È il motivo per
 * cui in M2 abbiamo insistito perché anche gli scarti degli estrattori
 * passassero da quel formato: qui si vede il ritorno di quell'investimento.
 */
async function call<T>(path: string, init?: RequestInit): Promise<T> {
	const response = await fetch(`${BASE}${path}`, init);

	if (!response.ok) {
		let code = 'unknown';
		let message = `HTTP ${response.status}`;
		try {
			const body = await response.json();
			code = body?.error?.code ?? code;
			message = body?.error?.message ?? message;
		} catch {
			// Risposta senza corpo JSON: teniamo il messaggio generico.
		}
		throw new ApiError(response.status, code, message);
	}

	if (response.status === 204) return undefined as T;
	return response.json() as Promise<T>;
}

export const api = {
	listInvoices: () => call<Invoice[]>('/api/invoices'),

	getInvoice: (id: string) => call<InvoiceDetail>(`/api/invoices/${id}`),

	uploadInvoice: (file: File) => {
		const form = new FormData();
		form.append('file', file);
		// Nessun Content-Type impostato a mano: il browser deve aggiungere da sé
		// il boundary del multipart, e impostarlo lo romperebbe.
		return call<{ id: string; status: InvoiceStatus }>('/api/invoices', {
			method: 'POST',
			body: form
		});
	},

	retryInvoice: (id: string) =>
		call<{ id: string; status: InvoiceStatus }>(`/api/invoices/${id}/retry`, { method: 'POST' }),

	invoiceFileUrl: (id: string) => `${BASE}/api/invoices/${id}/file`,

	listProducts: (params: { status?: ProductStatus; q?: string } = {}) => {
		const query = new URLSearchParams();
		if (params.status) query.set('status', params.status);
		if (params.q) query.set('q', params.q);
		const suffix = query.toString() ? `?${query}` : '';
		return call<ProductSummary[]>(`/api/products${suffix}`);
	},

	getProduct: (id: string) => call<Product>(`/api/products/${id}`),

	updateProduct: (id: string, body: { title?: string; description?: string | null }) =>
		call<Product>(`/api/products/${id}`, {
			method: 'PUT',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(body)
		}),

	deleteProduct: (id: string) => call<void>(`/api/products/${id}`, { method: 'DELETE' })
};
