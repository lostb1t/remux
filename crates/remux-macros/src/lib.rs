use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemFn, LitStr};

/// Internal: generates a handler function + inventory registration for a given HTTP method.
fn route_macro(method: &str, args: TokenStream, input: TokenStream) -> TokenStream {
    let path = parse_macro_input!(args as LitStr);
    let func = parse_macro_input!(input as ItemFn);

    let fn_name = &func.sig.ident;
    let vis = &func.vis;
    let register_fn = format_ident!("__register_route_{}", fn_name);
    let method_ident = format_ident!("{}", method);

    let output = quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #register_fn(router: ::axum::Router<crate::AppState>) -> ::axum::Router<crate::AppState> {
            router.route(#path, ::axum::routing::#method_ident(#fn_name))
        }

        ::inventory::submit! {
            crate::RouteRegistration(#register_fn)
        }
    };

    output.into()
}

/// Register a GET handler.
///
/// ```ignore
/// #[get("/system/ping")]
/// async fn system_ping() -> impl IntoResponse { "pong" }
/// ```
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro("get", args, input)
}

/// Register a POST handler.
#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro("post", args, input)
}

/// Register a PUT handler.
#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro("put", args, input)
}

/// Register a DELETE handler.
#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro("delete", args, input)
}

/// Register a PATCH handler.
#[proc_macro_attribute]
pub fn patch(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro("patch", args, input)
}

/// Register a handler for multiple HTTP methods on a path.
///
/// ```ignore
/// #[route("/hello/{name}", method = "GET", method = "POST")]
/// async fn hello(Path(name): Path<String>) -> impl IntoResponse {
///     format!("hello {name}")
/// }
/// ```
#[proc_macro_attribute]
pub fn route(args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);

    // Parse: "path", method = "GET", method = "POST", ...
    let args2: proc_macro2::TokenStream = args.into();
    let parsed = match syn::parse2::<RouteArgs>(args2) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };

    let path = &parsed.path;
    let fn_name = &func.sig.ident;
    let vis = &func.vis;
    let register_fn = format_ident!("__register_route_{}", fn_name);

    // Build chained method_router: get(handler).post(handler)...
    let methods: Vec<_> = parsed
        .methods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let method_ident = format_ident!("{}", m.value().to_lowercase());
            if i == 0 {
                quote! { ::axum::routing::#method_ident(#fn_name) }
            } else {
                quote! { .#method_ident(#fn_name) }
            }
        })
        .collect();

    let output = quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case)]
        #vis fn #register_fn(router: ::axum::Router<crate::AppState>) -> ::axum::Router<crate::AppState> {
            router.route(#path, #(#methods)*)
        }

        ::inventory::submit! {
            crate::RouteRegistration(#register_fn)
        }
    };

    output.into()
}

/// Parsed arguments for #[route("path", method = "GET", method = "POST")]
struct RouteArgs {
    path: LitStr,
    methods: Vec<LitStr>,
}

impl syn::parse::Parse for RouteArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let path: LitStr = input.parse()?;
        let mut methods = Vec::new();

        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let meta: syn::MetaNameValue = input.parse()?;
            if !meta.path.is_ident("method") {
                return Err(syn::Error::new_spanned(meta.path, "expected `method`"));
            }
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(lit),
                ..
            }) = meta.value
            {
                methods.push(lit);
            } else {
                return Err(syn::Error::new_spanned(
                    meta.value,
                    "expected string literal for method",
                ));
            }
        }

        if methods.is_empty() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "#[route] requires at least one `method = \"METHOD\"`",
            ));
        }

        Ok(RouteArgs { path, methods })
    }
}
