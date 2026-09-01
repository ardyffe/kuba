<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, type Product } from '$lib/api';
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';

	let prodotto = $state<Product | null>(null);
	let errore = $state<string | null>(null);
	let salvataggio = $state(false);
	let salvato = $state(false);

	// Le due copie modificabili. Restano separate dal prodotto caricato, così
	// si può sempre capire se c'è qualcosa di non salvato.
	let titolo = $state('');
	let descrizione = $state('');

	const id = $derived(page.params.id!);
	const modificato = $derived(
		!!prodotto && (titolo !== prodotto.title || descrizione !== (prodotto.description ?? ''))
	);

	async function carica() {
		try {
			prodotto = await api.getProduct(id);
			titolo = prodotto.title;
			descrizione = prodotto.description ?? '';
			errore = null;
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		}
	}

	$effect(() => {
		void id;
		carica();
	});

	async function salva() {
		salvataggio = true;
		errore = null;
		try {
			// Si manda solo ciò che è cambiato: è la semantica a tre stati del
			// backend. Una descrizione svuotata diventa `null` esplicito, che
			// significa "cancellala", non "non toccarla".
			const body: { title?: string; description?: string | null } = {};
			if (prodotto && titolo !== prodotto.title) body.title = titolo;
			if (prodotto && descrizione !== (prodotto.description ?? ''))
				body.description = descrizione.trim() === '' ? null : descrizione;

			prodotto = await api.updateProduct(id, body);
			titolo = prodotto.title;
			descrizione = prodotto.description ?? '';
			salvato = true;
			setTimeout(() => (salvato = false), 2000);
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		} finally {
			salvataggio = false;
		}
	}

	async function elimina() {
		if (!confirm('Eliminare questa scheda? Resta recuperabile dal filtro "Eliminati".')) return;
		try {
			await api.deleteProduct(id);
			goto('/products');
		} catch (e) {
			errore = e instanceof Error ? e.message : 'errore imprevisto';
		}
	}

	/** Le feature che l'agente ha compilato, escluse quelle di servizio. */
	const feature = $derived(
		Object.entries(prodotto?.attributes ?? {}).filter(
			([chiave, valore]) =>
				!chiave.startsWith('_') &&
				valore !== null &&
				!(Array.isArray(valore) && valore.length === 0)
		)
	);

	const confidenza = $derived(prodotto?.attributes?._confidenza as string | undefined);
	const fonti = $derived((prodotto?.attributes?._fonti as string[] | undefined) ?? []);
	const noteRevisione = $derived(prodotto?.attributes?._note_revisione as string | undefined);
</script>

<a href="/products" class="text-sm text-muted-foreground hover:text-foreground">← tutti i prodotti</a>

{#if errore && !prodotto}
	<div class="mt-6 rounded-md border border-destructive/40 bg-destructive/10 px-4 py-3 text-sm text-destructive">
		{errore}
	</div>
{:else if !prodotto}
	<p class="mt-6 text-sm text-muted-foreground">Caricamento…</p>
{:else}
	<div class="mt-3 mb-6 flex flex-wrap items-center gap-3">
		<h1 class="text-2xl font-semibold tracking-tight">{prodotto.title}</h1>
		<Badge value={prodotto.status} />
		{#if confidenza}
			<Badge
				value={confidenza === 'alta' ? 'done' : confidenza === 'media' ? 'pending' : 'failed'}
				label="confidenza {confidenza}"
			/>
		{/if}
	</div>

	{#if confidenza === 'bassa' || noteRevisione}
		<div class="mb-6 rounded-md border px-4 py-3 text-sm"
			 style="border-color: color-mix(in oklab, var(--status-review) 40%, transparent);
			        background-color: color-mix(in oklab, var(--status-review) 10%, transparent);">
			<strong>Da verificare.</strong>
			{noteRevisione ?? "L'agente non ha trovato fonti sufficienti: controlla le note olfattive prima di pubblicare."}
		</div>
	{/if}

	<div class="grid gap-6 lg:grid-cols-[2fr_1fr]">
		<div class="space-y-4">
			<div>
				<label for="titolo" class="mb-1.5 block text-sm font-medium">Titolo</label>
				<input
					id="titolo"
					bind:value={titolo}
					class="h-9 w-full rounded-md border border-input bg-transparent px-3 text-sm
					       focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				/>
			</div>

			<div>
				<label for="descrizione" class="mb-1.5 block text-sm font-medium">Descrizione (HTML)</label>
				<textarea
					id="descrizione"
					bind:value={descrizione}
					rows="14"
					class="w-full rounded-md border border-input bg-transparent px-3 py-2 font-mono text-xs leading-relaxed
					       focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				></textarea>
			</div>

			{#if descrizione}
				<div>
					<span class="mb-1.5 block text-sm font-medium">Anteprima</span>
					<div class="prose-sm rounded-md border border-border p-4 text-sm leading-relaxed [&_p]:mb-3">
						<!-- eslint-disable-next-line svelte/no-at-html-tags -->
						{@html descrizione}
					</div>
				</div>
			{/if}

			<div class="flex items-center gap-3">
				<Button onclick={salva} disabled={!modificato || salvataggio}>
					{salvataggio ? 'Salvataggio…' : 'Salva'}
				</Button>
				<Button variant="danger" onclick={elimina}>Elimina</Button>
				{#if salvato}
					<span class="text-sm text-[var(--status-ok)]">salvato</span>
				{:else if modificato}
					<span class="text-sm text-muted-foreground">modifiche non salvate</span>
				{/if}
				{#if errore}
					<span class="text-sm text-destructive">{errore}</span>
				{/if}
			</div>
		</div>

		<aside class="space-y-4 text-sm">
			<div class="rounded-lg border border-border p-4">
				<h2 class="mb-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">Dati</h2>
				<dl class="space-y-2">
					{#each [['EAN', prodotto.ean], ['SKU', prodotto.sku], ['Brand', prodotto.brand], ['Costo', prodotto.unit_cost], ['Prezzo', prodotto.price], ['Slug', prodotto.slug]] as [etichetta, valore] (etichetta)}
						<div class="flex justify-between gap-3">
							<dt class="text-muted-foreground">{etichetta}</dt>
							<dd class="text-right font-medium break-all">{valore ?? '—'}</dd>
						</div>
					{/each}
				</dl>
				{#if !prodotto.price}
					<p class="mt-3 text-xs text-muted-foreground">
						Il prezzo di vendita non arriva dalla fattura: va deciso a mano.
					</p>
				{/if}
			</div>

			{#if feature.length}
				<div class="rounded-lg border border-border p-4">
					<h2 class="mb-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">
						Caratteristiche
					</h2>
					<dl class="space-y-2">
						{#each feature as [chiave, valore] (chiave)}
							<div>
								<dt class="text-xs text-muted-foreground">{chiave.replaceAll('_', ' ')}</dt>
								<dd class="font-medium">
									{Array.isArray(valore) ? valore.join(', ') : String(valore)}
								</dd>
							</div>
						{/each}
					</dl>
				</div>
			{/if}

			{#if prodotto.categories.length}
				<div class="rounded-lg border border-border p-4">
					<h2 class="mb-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">
						Categorie
					</h2>
					<div class="flex flex-wrap gap-1.5">
						{#each prodotto.categories as categoria (categoria)}
							<span class="rounded-md bg-accent px-2 py-0.5 text-xs">{categoria}</span>
						{/each}
					</div>
				</div>
			{/if}

			{#if fonti.length}
				<div class="rounded-lg border border-border p-4">
					<h2 class="mb-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">
						Fonti consultate
					</h2>
					<ul class="space-y-1.5">
						{#each fonti as fonte (fonte)}
							<li>
								<a
									href={fonte}
									target="_blank"
									rel="noopener"
									class="text-xs break-all text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
								>
									{new URL(fonte).hostname}
								</a>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
		</aside>
	</div>
{/if}
