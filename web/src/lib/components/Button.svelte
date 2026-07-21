<script lang="ts">
  import type { Snippet } from 'svelte';

  type Variant = 'primary' | 'secondary' | 'outline' | 'ghost' | 'danger';
  type Size = 'sm' | 'md' | 'lg' | 'icon';

  type Props = {
    variant?: Variant;
    size?: Size;
    loading?: boolean;
    href?: string;
    class?: string;
    disabled?: boolean;
    children?: Snippet;
    [key: string]: unknown;
  };

  let {
    variant = 'primary',
    size = 'md',
    loading = false,
    href,
    class: klass = '',
    disabled = false,
    children,
    ...rest
  }: Props = $props();

  const base =
    'inline-flex items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap transition select-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50 focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:pointer-events-none disabled:opacity-50';

  const variants: Record<Variant, string> = {
    primary: 'bg-accent text-accent-ink hover:bg-accent-hover',
    secondary: 'border border-border bg-surface-2 text-text hover:border-border-strong',
    outline: 'border border-border bg-transparent text-text hover:bg-surface-2',
    ghost: 'bg-transparent text-muted hover:bg-surface-2 hover:text-text',
    danger: 'border border-danger/40 bg-transparent text-danger hover:bg-danger/10'
  };

  const sizes: Record<Size, string> = {
    sm: 'h-8 px-3 text-sm',
    md: 'h-9 px-4 text-sm',
    lg: 'h-10 px-5 text-sm',
    icon: 'h-9 w-9'
  };

  let cls = $derived(`${base} ${variants[variant]} ${sizes[size]} ${klass}`);
</script>

{#if href}
  <a {href} class={cls} {...rest}>
    {#if loading}<span class="spinner"></span>{/if}
    {@render children?.()}
  </a>
{:else}
  <button class={cls} disabled={disabled || loading} {...rest}>
    {#if loading}<span class="spinner"></span>{/if}
    {@render children?.()}
  </button>
{/if}
