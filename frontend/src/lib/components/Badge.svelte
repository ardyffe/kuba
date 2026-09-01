<script lang="ts">
	/**
	 * Il badge di stato.
	 *
	 * Prende il valore così com'è nel database (`in_progress`, `needs_review`...)
	 * e decide da sé colore ed etichetta. Le pagine non devono conoscere la
	 * mappatura: la scrivono una volta qui e la riusano ovunque.
	 */
	type Props = { value: string | null; label?: string };
	let { value, label }: Props = $props();

	const colori: Record<string, string> = {
		pending: 'var(--status-pending)',
		in_progress: 'var(--status-progress)',
		matched: 'var(--status-progress)',
		enriched: 'var(--status-progress)',
		succeeded: 'var(--status-ok)',
		done: 'var(--status-ok)',
		skip: 'var(--status-ok)',
		published: 'var(--status-ok)',
		failed: 'var(--status-fail)',
		deleted: 'var(--status-fail)',
		needs_review: 'var(--status-review)',
		create: 'var(--status-progress)',
		draft: 'var(--status-pending)'
	};

	const etichette: Record<string, string> = {
		pending: 'in coda',
		in_progress: 'in corso',
		succeeded: 'completata',
		failed: 'fallita',
		matched: 'abbinata',
		enriched: 'arricchita',
		done: 'fatta',
		skip: 'già a catalogo',
		create: 'da creare',
		needs_review: 'da rivedere',
		draft: 'bozza',
		published: 'pubblicato',
		deleted: 'eliminato',
		product: 'prodotto',
		shipping: 'spedizione',
		discount: 'sconto',
		unknown: 'non classificata'
	};

	const colore = $derived(value ? (colori[value] ?? 'var(--muted-foreground)') : 'var(--muted-foreground)');
	const testo = $derived(label ?? (value ? (etichette[value] ?? value) : '—'));
</script>

<span
	class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium whitespace-nowrap"
	style="border-color: color-mix(in oklab, {colore} 35%, transparent);
	       background-color: color-mix(in oklab, {colore} 12%, transparent);
	       color: {colore};"
>
	<span class="size-1.5 rounded-full" style="background-color: {colore}"></span>
	{testo}
</span>
