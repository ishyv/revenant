
// this file is generated — do not edit it


declare module "svelte/elements" {
	export interface HTMLAttributes<T> {
		'data-sveltekit-keepfocus'?: true | '' | 'off' | undefined | null;
		'data-sveltekit-noscroll'?: true | '' | 'off' | undefined | null;
		'data-sveltekit-preload-code'?:
			| true
			| ''
			| 'eager'
			| 'viewport'
			| 'hover'
			| 'tap'
			| 'off'
			| undefined
			| null;
		'data-sveltekit-preload-data'?: true | '' | 'hover' | 'tap' | 'off' | undefined | null;
		'data-sveltekit-reload'?: true | '' | 'off' | undefined | null;
		'data-sveltekit-replacestate'?: true | '' | 'off' | undefined | null;
	}
}

export {};


declare module "$app/types" {
	export interface AppTypes {
		RouteId(): "/";
		RouteParams(): {
			
		};
		LayoutParams(): {
			"/": Record<string, never>
		};
		Pathname(): "/";
		ResolvedPathname(): `${"" | `/${string}`}${ReturnType<AppTypes['Pathname']>}`;
		Asset(): "/et-book/et-book-bold-line-figures/et-book-bold-line-figures.eot" | "/et-book/et-book-bold-line-figures/et-book-bold-line-figures.svg" | "/et-book/et-book-bold-line-figures/et-book-bold-line-figures.ttf" | "/et-book/et-book-bold-line-figures/et-book-bold-line-figures.woff" | "/et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.eot" | "/et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.svg" | "/et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.ttf" | "/et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.woff" | "/et-book/et-book-roman-line-figures/et-book-roman-line-figures.eot" | "/et-book/et-book-roman-line-figures/et-book-roman-line-figures.svg" | "/et-book/et-book-roman-line-figures/et-book-roman-line-figures.ttf" | "/et-book/et-book-roman-line-figures/et-book-roman-line-figures.woff" | "/et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.eot" | "/et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.svg" | "/et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.ttf" | "/et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.woff" | "/et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.eot" | "/et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.svg" | "/et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.ttf" | "/et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.woff" | "/robots.txt" | string & {};
	}
}