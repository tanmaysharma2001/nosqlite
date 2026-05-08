//! `#[document]` attribute macro for the [nosqlite](https://crates.io/crates/nosqlite)
//! document database.
//!
//! ```ignore
//! use nosqlite::document;
//!
//! #[document]
//! struct User {
//!     #[id]
//!     id: Option<String>,
//!     name: String,
//!     age: u32,
//! }
//! ```
//!
//! The macro:
//!
//! - Derives `serde::Serialize` and `serde::Deserialize`.
//! - Renames the `#[id]`-marked field to `_id` on the wire and skips it
//!   when serializing if `None`, so SQLite can generate a fresh ULID.
//! - Implements the `nosqlite::Document` trait, exposing `id()` and
//!   `set_id(...)`.
//!
//! If no field is marked `#[id]` but a field is named `id`, that field is
//! used. The id field must be `Option<String>` (so a freshly-constructed
//! document can leave the id blank).

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Attribute, Fields, ItemStruct};

#[proc_macro_attribute]
pub fn document(_args: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemStruct);

    let id_ident = match prepare_id_field(&mut input) {
        Ok(name) => name,
        Err(e) => return e.to_compile_error().into(),
    };

    let name = &input.ident;
    let (impl_g, ty_g, where_clause) = input.generics.split_for_impl();

    let expanded = quote! {
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        #input

        impl #impl_g ::nosqlite::Document for #name #ty_g #where_clause {
            fn id(&self) -> ::std::option::Option<&::std::primitive::str> {
                self.#id_ident.as_deref()
            }
            fn set_id(&mut self, id: ::std::string::String) {
                self.#id_ident = ::std::option::Option::Some(id);
            }
        }
    };

    expanded.into()
}

fn prepare_id_field(input: &mut ItemStruct) -> syn::Result<syn::Ident> {
    let fields = match &mut input.fields {
        Fields::Named(named) => &mut named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "#[document] requires a struct with named fields",
            ));
        }
    };

    // Pass 1: find a field carrying `#[id]` and strip the attribute.
    let mut tagged: Option<usize> = None;
    for (i, field) in fields.iter_mut().enumerate() {
        let mut has_id = false;
        field.attrs.retain(|attr| {
            if attr.path().is_ident("id") {
                has_id = true;
                false
            } else {
                true
            }
        });
        if has_id {
            if tagged.is_some() {
                return Err(syn::Error::new_spanned(
                    field,
                    "only one field may be marked #[id]",
                ));
            }
            tagged = Some(i);
        }
    }

    // Pass 2: fall back to a field named `id`.
    let idx = match tagged {
        Some(i) => i,
        None => fields
            .iter()
            .position(|f| f.ident.as_ref().is_some_and(|n| n == "id"))
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    &input.ident,
                    "#[document] requires a field marked #[id] or named `id`",
                )
            })?,
    };

    let id_field = fields.iter_mut().nth(idx).unwrap();

    if !is_option_string(&id_field.ty) {
        return Err(syn::Error::new_spanned(
            &id_field.ty,
            "#[document] id field must be `Option<String>`",
        ));
    }

    // Add the serde rename if not already present.
    let already_has_serde_rename = id_field.attrs.iter().any(has_serde_rename);
    if !already_has_serde_rename {
        let attr: Attribute = syn::parse_quote!(
            #[serde(rename = "_id", default, skip_serializing_if = "::std::option::Option::is_none")]
        );
        id_field.attrs.push(attr);
    }

    Ok(id_field.ident.clone().unwrap())
}

fn has_serde_rename(attr: &Attribute) -> bool {
    if !attr.path().is_ident("serde") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("rename") {
            found = true;
        }
        Ok(())
    });
    found
}

fn is_option_string(ty: &syn::Type) -> bool {
    let path = match ty {
        syn::Type::Path(p) => &p.path,
        _ => return false,
    };
    let last = match path.segments.last() {
        Some(s) => s,
        None => return false,
    };
    if last.ident != "Option" {
        return false;
    }
    let args = match &last.arguments {
        syn::PathArguments::AngleBracketed(a) => a,
        _ => return false,
    };
    let inner = match args.args.first() {
        Some(syn::GenericArgument::Type(t)) => t,
        _ => return false,
    };
    let inner_path = match inner {
        syn::Type::Path(p) => &p.path,
        _ => return false,
    };
    inner_path
        .segments
        .last()
        .map(|s| s.ident == "String")
        .unwrap_or(false)
}
