/// Build parallel vectors of `(names, functions)` from `name: callable` pairs.
///
/// - Use the typed form when you want explicit control over the value type:
///   ```rust
///   let (names, funcs): (Vec<String>, Vec<Step<'program>>) = named_function_vec![
///       @type Step<'program>;
///       create_root: |ctx: &AppContext| -> std::io::Result<()> { /* ... */ },
///       verify_packages: |ctx: &AppContext| -> std::io::Result<()> { /* ... */ },
///   ];
///   ```
/// - The default form assumes you’re calling it inside a function that defines
///   a `'program` lifetime (e.g., `fn build<'program>(...)`), so that the
///   boxed callable type can include `'program`.
///
/// This pattern makes it easy to keep step names synchronized with their
/// implementations for reporting and error handling.
#[macro_export]
macro_rules! named_function_vec {
    // Typed form: values are coerced into the provided type (typically a boxed trait object).
    (@type $fty:ty; $($name:ident: $func:expr),+ $(,)?) => {
        {
        // Collect step names as Strings.
        let names: ::std::vec::Vec<::std::string::String> = ::std::vec![
            $( ::std::string::String::from(::std::stringify!($name)) ),+
        ];

        // Box each callable expression into the requested type.
        // Works for closures that capture environment (e.g., use of `app_context`).
        let values: ::std::vec::Vec<$fty> = ::std::vec![
            $( ::std::boxed::Box::new($func) as $fty ),+
        ];

        (names, values)
        }
    };

    // Default form: assumes `AppContext` and `'program` are in scope at call-site.
    ($($name:ident: $func:expr),+ $(,)?) => {
        {
        $crate::named_function_vec![
            @type ::std::boxed::Box<dyn Fn(&AppContext) -> 
                  ::std::io::Result<()> + Send + Sync + 'program>;
                  
            $( $name : $func ),+
        ]
        }
    };
}

#[macro_export]
/// Defines a strongly-typed, file-backed text template with a `render` method.
///
/// - Generates a zero-sized struct `Name` with:
///   - `const CONTENT: &str` = include_str!(path)
///   - `fn render(<fields>) -> String` that replaces `{{field}}` placeholders
///     with provided values (exact string match).
/// - The function signature enforces that all declared fields are provided at
///   call sites (compile-time), avoiding ad-hoc maps.
/// - With no fields, `render()` returns the template unchanged.
///
/// Example:
///   define_template! {
///       pub Template RevenantGlobal {
///           path: "src/templates/revenant.global.template.ts",
///           version: &str,
///           root: &str,
///           svelte: &str,
///           compiler: &str,
///       }
///   }
macro_rules! define_template {
    // With fields: generate render(args...)
    (
        $(#[$meta:meta])*
        $vis:vis Template $Name:ident {
            path: $path:expr,
            $( $field:ident : $fty:ty ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $Name;

        impl $Name {
            pub const CONTENT: &'static str = ::core::include_str!($path);

            #[allow(clippy::needless_pass_by_value)]
            pub fn render( $( $field : $fty ),+ ) -> ::std::string::String {
                let mut s = ::std::string::String::from(Self::CONTENT);
                $(
                    let ph = ::std::string::String::from("{{") + ::std::stringify!($field) + "}}";
                    s = s.replace(&ph, $field);
                )+
                #[cfg(debug_assertions)]
                {
                    if let Some(idx) = s.find("{{") {
                        eprintln!("[revenant] warning: unreplaced placeholder near byte {} in {}", idx, ::std::stringify!($Name));
                    }
                }
                s
            }
        }
    };

    // No fields: render() returns content unchanged (no mut warning)
    (
        $(#[$meta:meta])*
        $vis:vis Template $Name:ident {
            path: $path:expr $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $Name;

        impl $Name {
            pub const CONTENT: &'static str = ::core::include_str!($path);

            pub fn render() -> ::std::string::String {
                ::std::string::String::from(Self::CONTENT)
            }
        }
    };
}
