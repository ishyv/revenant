/// Build parallel vectors of (names, functions) from `name: callable` pairs.
/// Supports closures that capture environment by accepting `$func:expr`.
/// The typed arm boxes each callable into the requested type (e.g., `Step<'a>`).
///
/// Usage:
/// ```rust
/// let (names, funcs): (Vec<String>, Vec<Step<'program>>) = named_function_vec![
///     @type Step<'program>;
///     create_root: |ctx: &AppContext| -> std::io::Result<()> { /* ... */ },
///     verify_packages: |ctx: &AppContext| -> std::io::Result<()> { /* ... */ },
/// ];
/// ```
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

    // Default form: assume a common boxed callable type used in your project.
    // Adjust the default to your actual alias if you want implicit usage.
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
