<script lang="ts">
	import { api, ApiError, type Invoice } from '$lib/api';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';

	let fatture = $state<Invoice[]>([]);
	let errore = $state<string | null>(null);
	let caricamento = $state(false);
	let trascinamento = $state(false);
	let primoCaricamento = $state(true);

	/**
	 * Finché una fattura è in coda o in lavorazione, la pagina si aggiorna da
	 * sola. Quando non c'è più niente in movimento, il polling si ferma: non
	 * ha senso interrogare il server ogni due secondi per vedere sempre lo
	 * stesso elenco.
	 *
	 * Da M7 questo diventerà una connessione SSE — il server dirà lui quando
	 * qualcosa cambia, invece di farselo chiedere.
	 */
	const inMovimento = $derived(
		fatture.some((f) => f.status === 'pending' || f.status === 'in_progress')
	);

	async function ricarica() {
		try {
			fatture = await api.listInvoices();
			errore = null;
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		} finally {
			primoCaricamento = false;
		}
	}

	$effect(() => {
		ricarica();
	});

	$effect(() => {
		if (!inMovimento) return;
		const timer = setInterval(ricarica, 2000);
		// La funzione restituita da `$effect` è la pulizia: Svelte la chiama
		// quando l'effetto si ri-esegue o il componente sparisce. Senza,
		// resterebbero timer appesi a ogni cambio di stato.
		return () => clearInterval(timer);
	});

	async function carica(file: File) {
		caricamento = true;
		errore = null;
		try {
			await api.uploadInvoice(file);
			await ricarica();
		} catch (e) {
			errore =
				e instanceof ApiError && e.code === 'duplicate_invoice'
					? e.message
					: e instanceof Error
						? e.message
						: 'errore imprevisto';
		} finally {
			caricamento = false;
		}
	}

	function suFile(event: Event) {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		if (file) carica(file);
		// Azzera, altrimenti ricaricare lo stesso file non scatena l'evento.
		input.value = '';
	}

	function suDrop(event: DragEvent) {
		event.preventDefault();
		trascinamento = false;
		const file = event.dataTransfer?.files?.[0];
		if (file) carica(file);
	}

	async function riprova(id: string) {
		try {
			await api.retryInvoice(id);
			await ricarica();
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		}
	}

	const dataOra = (iso: string) =>
		new Date(iso).toLocaleString('it-IT', { dateStyle: 'short', timeStyle: 'short' });
</script>

<div class="mb-8 flex items-end justify-between">
	<div>
		<h1 class="text-2xl font-semibold tracking-tight">Fatture</h1>
		<p class="mt-1 text-sm text-muted-foreground">
			Carica una fattura in PDF: l'agente ne estrae le righe e genera le schede dei prodotti
			non ancora a catalogo.
		</p>
	</div>
	{#if inMovimento}
		<span class="flex items-center gap-2 text-xs text-muted-foreground">
			<span class="size-1.5 animate-pulse rounded-full bg-[var(--status-progress)]"></span>
			lavorazione in corso
		</span>
	{/if}
</div>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<label
	class="mb-6 flex cursor-pointer flex-col items-center justify-center rounded-lg border border-dashed
	       px-6 py-10 text-center transition
	       {trascinamento ? 'border-primary bg-accent' : 'border-border hover:bg-accent/50'}"
	ondragover={(e) => {
		e.preventDefault();
		trascinamento = true;
	}}
	ondragleave={() => (trascinamento = false)}
	ondrop={suDrop}
>
	<input type="file" accept="application/pdf" class="hidden" onchange={suFile} disabled={caricamento} />
	<span class="text-sm font-medium">
		{caricamento ? 'Caricamento…' : 'Trascina qui il PDF, o clicca per sceglierlo'}
	</span>
	<span class="mt-1 text-xs text-muted-foreground">Solo PDF, massimo 20 MB</span>
</label>

{#if errore}
	<div
		class="mb-6 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive"
	>
		{errore}
	</div>
{/if}

<div class="overflow-hidden rounded-lg border border-border">
	<table class="w-full text-sm">
		<thead class="bg-muted/60 text-left text-xs text-muted-foreground">
			<tr>
				<th class="px-4 py-2.5 font-medium">File</th>
				<th class="px-4 py-2.5 font-medium">Fornitore</th>
				<th class="px-4 py-2.5 font-medium">Numero</th>
				<th class="px-4 py-2.5 font-medium text-right">Totale</th>
				<th class="px-4 py-2.5 font-medium">Stato</th>
				<th class="px-4 py-2.5 font-medium">Caricata</th>
				<th class="px-4 py-2.5"></th>
			</tr>
		</thead>
		<tbody>
			{#each fatture as fattura (fattura.id)}
				<tr class="border-t border-border hover:bg-accent/40">
					<td class="px-4 py-3">
						<a class="font-medium hover:underline" href="/invoices/{fattura.id}">
							{fattura.original_filename}
						</a>
						{#if fattura.error_message}
							<div class="mt-0.5 text-xs text-destructive">{fattura.error_message}</div>
						{/if}
					</td>
					<td class="px-4 py-3 text-muted-foreground">{fattura.supplier_name ?? '—'}</td>
					<td class="px-4 py-3 text-muted-foreground">{fattura.invoice_number ?? '—'}</td>
					<td class="px-4 py-3 text-right tabular-nums">
						{fattura.total_amount ? `${fattura.total_amount} ${fattura.currency ?? ''}` : '—'}
					</td>
					<td class="px-4 py-3"><Badge value={fattura.status} /></td>
					<td class="px-4 py-3 text-xs text-muted-foreground">{dataOra(fattura.uploaded_at)}</td>
					<td class="px-4 py-3 text-right">
						{#if fattura.status === 'failed'}
							<Button size="sm" variant="ghost" onclick={() => riprova(fattura.id)}>
								Riprova
							</Button>
						{/if}
					</td>
				</tr>
			{:else}
				<tr>
					<td colspan="7" class="px-4 py-10 text-center text-sm text-muted-foreground">
						{primoCaricamento ? 'Caricamento…' : 'Nessuna fattura. Caricane una qui sopra.'}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
