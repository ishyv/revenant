import { Revenant as R } from '$lib/revenant.global';
// Make it available at runtime in the browser
// eslint-disable-next-line @typescript-eslint/no-explicit-any
(globalThis as any).Revenant = R;
