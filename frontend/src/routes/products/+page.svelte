<script lang="ts">
	import { api, type ProductSummary, type ProductStatus } from '$lib/api';
	import Badge from '$lib/components/Badge.svelte';

	let prodotti = $state<ProductSummary[]>([]);
	let errore = $state<string | null>(null);
	let ricerca = $state('');
	let filtro = $state<ProductStatus | ''>('');
	let caricato = $state(false);

	const stati: Array<{ valore: ProductStatus | ''; etichetta: string }> = [
		{ valore: '', etichetta: 'Tutti' },
		{ valore: 'draft', etichetta: 'Bozze' },
		{ valore: 'published', etichetta: 'Pubblicati' },
		{ valore: 'deleted', etichetta: 'Eliminati' }
	];

	async function ricarica() {
		try {
			prodotti = await api.listProducts({
				status: filtro || undefined,
				q: ricerca.trim() || undefined
			});
			errore = null;
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		} finally {
			caricato = true;
		}
	}

	// L'effetto legge `ricerca` e `filtro`: cambiandoli si ri-esegue da solo,
	// senza doverlo richiamare a mano da ogni handler.
	$effect(() => {
		void ricerca;
		void filtro;
		const timer = setTimeout(ricarica, 200); // piccolo debounce sulla ricerca
		return () => clearTimeout(timer);
	});
</script>

<div class="mb-6">
	<h1 class="text-2xl font-semibold tracking-tight">Prodotti</h1>
	<p class="mt-1 text-sm text-muted-foreground">
		Le schede generate dall'agente nascono come bozze: qui si rivedono prima della pubblicazione.
	</p>
</div>

<div class="mb-4 flex flex-wrap items-center gap-3">
	<input
		bind:value={ricerca}
		placeholder="Cerca per titolo, EAN o SKU…"
		class="h-9 w-72 rounded-md border border-input bg-transparent px-3 text-sm
		       focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
	/>
	<div class="flex gap-1">
		{#each stati as stato (stato.etichetta)}
			<button
				onclick={() => (filtro = stato.valore)}
				class="cursor-pointer rounded-md px-3 py-1.5 text-sm transition
				       {filtro === stato.valore ? 'bg-accent font-medium' : 'text-muted-foreground hover:text-foreground'}"
			>
				{stato.etichetta}
			</button>
		{/each}
	</div>
	<span class="ml-auto text-xs text-muted-foreground">
		{prodotti.length}
		{prodotti.length === 1 ? 'risultato' : 'risultati'}
	</span>
</div>

{#if errore}
	<div class="mb-6 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
		{errore}
	</div>
{/if}

<div class="overflow-hidden rounded-lg border border-border">
	<table class="w-full text-sm">
		<thead class="bg-muted/60 text-left text-xs text-muted-foreground">
			<tr>
				<th class="px-4 py-2.5 font-medium">Titolo</th>
				<th class="px-4 py-2.5 font-medium">Brand</th>
				<th class="px-4 py-2.5 font-medium">EAN</th>
				<th class="px-4 py-2.5 font-medium text-right">Prezzo</th>
				<th class="px-4 py-2.5 font-medium">Stato</th>
			</tr>
		</thead>
		<tbody>
			{#each prodotti as prodotto (prodotto.id)}
				<tr class="border-t border-border hover:bg-accent/40">
					<td class="px-4 py-3">
						<a class="font-medium hover:underline" href="/products/{prodotto.id}">
							{prodotto.title}
						</a>
					</td>
					<td class="px-4 py-3 text-muted-foreground">{prodotto.brand ?? '—'}</td>
					<td class="px-4 py-3 font-mono text-xs text-muted-foreground">{prodotto.ean ?? '—'}</td>
					<td class="px-4 py-3 text-right tabular-nums">{prodotto.price ?? '—'}</td>
					<td class="px-4 py-3"><Badge value={prodotto.status} /></td>
				</tr>
			{:else}
				<tr>
					<td colspan="5" class="px-4 py-10 text-center text-sm text-muted-foreground">
						{caricato ? 'Nessun prodotto trovato.' : 'Caricamento…'}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
