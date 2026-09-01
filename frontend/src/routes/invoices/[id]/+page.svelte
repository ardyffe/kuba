<script lang="ts">
	import { page } from '$app/state';
	import { api, type InvoiceDetail } from '$lib/api';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';

	let fattura = $state<InvoiceDetail | null>(null);
	let errore = $state<string | null>(null);

	const id = $derived(page.params.id!);

	const inMovimento = $derived(
		fattura?.status === 'pending' || fattura?.status === 'in_progress'
	);

	async function ricarica() {
		try {
			fattura = await api.getInvoice(id);
			errore = null;
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		}
	}

	$effect(() => {
		// Leggere `id` qui dentro lega l'effetto al parametro: se si naviga da
		// una fattura all'altra, l'effetto si ri-esegue da solo.
		void id;
		ricarica();
	});

	$effect(() => {
		if (!inMovimento) return;
		const timer = setInterval(ricarica, 2000);
		return () => clearInterval(timer);
	});

	/** Le righe raggruppate per azione, per far vedere prima quelle che
	 *  richiedono un intervento umano. */
	const daRivedere = $derived(fattura?.lines.filter((l) => l.action === 'needs_review') ?? []);
	const create = $derived(fattura?.lines.filter((l) => l.action === 'create') ?? []);
	const saltate = $derived(fattura?.lines.filter((l) => l.action === 'skip') ?? []);
</script>

<a href="/" class="text-sm text-muted-foreground hover:text-foreground">← tutte le fatture</a>

{#if errore}
	<div class="mt-6 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
		{errore}
	</div>
{:else if !fattura}
	<p class="mt-6 text-sm text-muted-foreground">Caricamento…</p>
{:else}
	<div class="mt-3 mb-6 flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold tracking-tight">{fattura.original_filename}</h1>
		<Badge value={fattura.status} />
		<a
			href={api.invoiceFileUrl(fattura.id)}
			target="_blank"
			rel="noopener"
			class="ml-auto text-sm text-muted-foreground underline-offset-4 hover:underline"
		>
			apri il PDF originale
		</a>
		{#if fattura.status === 'failed'}
			<Button size="sm" variant="ghost" onclick={async () => { await api.retryInvoice(id); ricarica(); }}>
				Riprova
			</Button>
		{/if}
	</div>

	{#if fattura.error_message}
		<div class="mb-6 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
			{fattura.error_message}
		</div>
	{/if}

	<dl class="mb-8 grid grid-cols-2 gap-4 rounded-lg border border-border p-4 text-sm sm:grid-cols-4">
		{#each [['Fornitore', fattura.supplier_name], ['Numero', fattura.invoice_number], ['Data', fattura.invoice_date], ['Totale', fattura.total_amount ? `${fattura.total_amount} ${fattura.currency ?? ''}` : null]] as [etichetta, valore] (etichetta)}
			<div>
				<dt class="text-xs text-muted-foreground">{etichetta}</dt>
				<dd class="mt-0.5 font-medium">{valore ?? '—'}</dd>
			</div>
		{/each}
	</dl>

	<div class="mb-4 flex flex-wrap gap-4 text-sm">
		<span><strong>{fattura.lines.length}</strong> righe</span>
		<span class="text-muted-foreground">·</span>
		<span><strong>{create.length}</strong> da creare</span>
		<span><strong>{saltate.length}</strong> già a catalogo</span>
		<span><strong>{daRivedere.length}</strong> da rivedere</span>
	</div>

	<div class="overflow-hidden rounded-lg border border-border">
		<table class="w-full text-sm">
			<thead class="bg-muted/60 text-left text-xs text-muted-foreground">
				<tr>
					<th class="px-3 py-2.5 font-medium">#</th>
					<th class="px-3 py-2.5 font-medium">Descrizione</th>
					<th class="px-3 py-2.5 font-medium">EAN</th>
					<th class="px-3 py-2.5 font-medium text-right">Qtà</th>
					<th class="px-3 py-2.5 font-medium text-right">Prezzo</th>
					<th class="px-3 py-2.5 font-medium">Tipo</th>
					<th class="px-3 py-2.5 font-medium">Azione</th>
					<th class="px-3 py-2.5 font-medium">Esito</th>
				</tr>
			</thead>
			<tbody>
				{#each fattura.lines as riga (riga.id)}
					<tr class="border-t border-border align-top hover:bg-accent/40">
						<td class="px-3 py-2.5 text-muted-foreground tabular-nums">{riga.line_no}</td>
						<td class="px-3 py-2.5">
							{#if riga.matched_product_id}
								<a class="font-medium hover:underline" href="/products/{riga.matched_product_id}">
									{riga.description ?? riga.raw_text}
								</a>
							{:else}
								<span class="font-medium">{riga.description ?? riga.raw_text}</span>
							{/if}
							{#if riga.error_message}
								<div class="mt-0.5 text-xs text-[var(--status-review)]">{riga.error_message}</div>
							{/if}
						</td>
						<td class="px-3 py-2.5 font-mono text-xs text-muted-foreground">{riga.ean ?? '—'}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{riga.quantity ?? '—'}</td>
						<td class="px-3 py-2.5 text-right tabular-nums">{riga.unit_price ?? '—'}</td>
						<td class="px-3 py-2.5"><Badge value={riga.kind} /></td>
						<td class="px-3 py-2.5"><Badge value={riga.action} /></td>
						<td class="px-3 py-2.5"><Badge value={riga.status} /></td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/if}
