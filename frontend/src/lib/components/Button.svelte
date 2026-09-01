<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	type Props = HTMLButtonAttributes & {
		variant?: 'primary' | 'ghost' | 'danger';
		size?: 'sm' | 'md';
		children: Snippet;
	};

	let { variant = 'primary', size = 'md', children, class: extra = '', ...rest }: Props = $props();

	const varianti = {
		primary: 'bg-primary text-primary-foreground hover:opacity-90',
		ghost: 'bg-transparent hover:bg-accent border border-border',
		danger: 'bg-transparent text-destructive border border-destructive/40 hover:bg-destructive/10'
	};

	const dimensioni = { sm: 'h-8 px-3 text-xs', md: 'h-9 px-4 text-sm' };
</script>

<button
	class="inline-flex cursor-pointer items-center justify-center gap-2 rounded-md font-medium
	       transition disabled:pointer-events-none disabled:opacity-50
	       focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none
	       {varianti[variant]} {dimensioni[size]} {extra}"
	{...rest}
>
	{@render children()}
</button>
