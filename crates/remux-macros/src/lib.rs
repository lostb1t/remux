use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote, ToTokens};
use syn::{parse_macro_input, Fields, ItemFn, ItemStruct, LitStr};

struct MultiPath(Vec<LitStr>);

impl syn::parse::Parse for MultiPath {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut paths = vec![input.parse::<LitStr>()?];
        while input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
            if input.is_empty() {
                break;
            }
            // Stop if next token looks like `method = ...` (for #[route])
            if input.peek(syn::Ident) {
                break;
            }
            paths.push(input.parse::<LitStr>()?);
        }
        Ok(MultiPath(paths))
    }
}

/// Internal: generates a handler function + inventory registration(s) for a given HTTP method.
/// Accepts one or more paths: `#[get("/a", "/b")]` registers both.
fn route_macro(method: &str, args: TokenStream, input: TokenStream) -> TokenStream {
    let MultiPath(paths) = parse_macro_input!(args as MultiPath);
    let func = parse_macro_input!(input as ItemFn);

    let fn_name = &func.sig.ident;
    let vis = &func.vis;
    let method_ident = format_ident!("{}", method);

    let registrations = paths.iter().enumerate().map(|(i, path)| {
        let register_fn = if i == 0 {
            format_ident!("__register_route_{}", fn_name)
        } else {
            format_ident!("__register_route_{}_{}", fn_name, i)
        };
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            #vis fn #register_fn(router: ::axum::Router<crate::AppState>) -> ::axum::Router<crate::AppState> {
                router.route(#path, ::axum::routing::#method_ident(#fn_name))
            }

            ::inventory::submit! {
                crate::RouteRegistration(#register_fn)
            }
        }
    });

    let output = quote! {
        #func
        #(#registrations)*
    };

    output.into()
}

/// Register a GET handler. Accepts multiple paths: `#[get("/a", "/b")]`.
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

/// Attribute macro that makes a query-parameter struct case-insensitive.
///
/// Adds `#[serde(alias = "...")]` attributes for the camelCase, PascalCase,
/// lowercase, and SCREAMING_SNAKE_CASE variants of every field name.
/// Also strips any struct-level `#[serde(rename_all = "...")]` and
/// injects `#[derive(serde::Deserialize)]` if not already present.
///
/// ```ignore
/// #[api_query]
/// pub struct AddItemsQuery {
///     #[serde(default)]
///     pub ids: CommaSeparatedList<Uuid>,
/// }
/// ```
#[proc_macro_attribute]
pub fn api_query(_args: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemStruct);

    // Check whether Deserialize is already derived so we don't add it twice.
    let has_deser = item.attrs.iter().any(|a| {
        a.path().is_ident("derive")
            && a.to_token_stream().to_string().contains("Deserialize")
    });

    // Drop struct-level #[serde(rename_all = "...")] — aliases cover all cases.
    item.attrs.retain(|a| {
        if !a.path().is_ident("serde") {
            return true;
        }
        !a.to_token_stream().to_string().contains("rename_all")
    });

    // Inject per-field aliases.
    if let Fields::Named(ref mut fields) = item.fields {
        for field in &mut fields.named {
            let Some(ident) = &field.ident else { continue };
            for variant in query_field_aliases(&ident.to_string()) {
                let lit = LitStr::new(&variant, Span::call_site());
                field.attrs.push(syn::parse_quote!(#[serde(alias = #lit)]));
            }
        }
    }

    let derive_deser = if has_deser {
        quote! {}
    } else {
        quote! { #[derive(serde::Deserialize)] }
    };

    quote! {
        #derive_deser
        #item
    }
    .into()
}

/// Returns the case variants of a snake_case field name that differ from the
/// original, so they can be registered as serde aliases.
fn query_field_aliases(snake: &str) -> Vec<String> {
    let words: Vec<&str> = snake.split('_').collect();

    let camel = {
        let mut s = words[0].to_lowercase();
        for w in &words[1..] {
            let mut chars = w.chars();
            if let Some(f) = chars.next() {
                s.extend(f.to_uppercase());
                s.push_str(chars.as_str());
            }
        }
        s
    };

    let pascal: String = words
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect();

    let lower: String = snake
        .chars()
        .filter(|&c| c != '_')
        .collect::<String>()
        .to_lowercase();

    let screaming = snake.to_uppercase();

    let mut variants = vec![camel, pascal, lower, screaming];
    variants.sort();
    variants.dedup();
    variants.retain(|v| v != snake);
    variants
}
