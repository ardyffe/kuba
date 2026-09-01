<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';

	let { children } = $props();

	const voci = [
		{ href: '/', label: 'Fatture' },
		{ href: '/products', label: 'Prodotti' }
	];

	// `page` è una rune di SvelteKit 5: si legge come un valore normale e si
	// aggiorna da sé a ogni navigazione.
	const attiva = (href: string) =>
		href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
</script>

<svelte:head>
	<title>kuba</title>
</svelte:head>

<div class="min-h-screen">
	<header class="border-b border-border">
		<div class="mx-auto flex max-w-6xl items-center gap-6 px-6 py-3">
			<a href="/" class="text-sm font-semibold tracking-tight">kuba</a>
			<nav class="flex gap-1">
				{#each voci as voce (voce.href)}
					<a
						href={voce.href}
						class="rounded-md px-3 py-1.5 text-sm transition
						       {attiva(voce.href)
							? 'bg-accent font-medium'
							: 'text-muted-foreground hover:text-foreground'}"
					>
						{voce.label}
					</a>
				{/each}
			</nav>
			<span class="ml-auto text-xs text-muted-foreground">
				da fattura a scheda prodotto
			</span>
		</div>
	</header>

	<main class="mx-auto max-w-6xl px-6 py-8">
		{@render children()}
	</main>
</div>
