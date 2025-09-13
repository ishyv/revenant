//! Text templates used to generate files in the Svelte project.
//! Keeping them out of Rust string literals makes maintenance easier and
//! allows compile-time enforcement of required fields via the template macro.

use crate::define_template;

// Strongly-typed templates (placeholders are `{{name}}`).
define_template! {
    /// Template for `output/svelte/src/lib/revenant.global.ts`
    /// Required placeholders: {{version}}, {{root}}, {{svelte}}, {{compiler}}
    pub Template RevenantGlobal {
        path: "revenant.global.template.ts",
        version: &str,
        root: &str,
        svelte: &str,
        compiler: &str,
    }
}

define_template! {
    /// Template for `output/svelte/src/app.d.ts`
    pub Template AppDTs {
        path: "app.d.template.ts",
    }
}

define_template! {
    /// Template for `output/svelte/src/hooks.client.ts`
    pub Template HooksClientTs {
        path: "hooks.client.template.ts",
    }
}
