//! `#[derive(Params)]`: turns a struct into a command's parameter list.
//!
//! Each named field becomes one parameter. The field's type decides the
//! parameter type (`bool`, `i64`, `f64`, `String`, `PathBuf`, `DatasetId`,
//! `Vec<T>`; wrap in `Option` to make it optional) and its doc comment
//! becomes the help text. Further details go in `#[param(...)]`:
//!
//! | Key | Meaning |
//! |---|---|
//! | `label = "..."` | Dialog label (default: the field name, humanised) |
//! | `default = <literal>` | Default value |
//! | `min = <n>`, `max = <n>`, `step = <n>` | Numeric bounds and increment |
//! | `unit = "..."` | Unit shown after the control |
//! | `choices("a", "b")` | Restrict a `String` to these values |
//! | `path = "open" \| "save" \| "dir"` | What a `PathBuf` is used for |
//! | `ijm = "..."` | Keyword in ImageJ macro option strings |
//! | `position = "channel" \| "slice" \| "frame" \| "stack"` | The parameter names a plane (or asks for all of them); frontends fill it from the view |
//! | `advanced` | Show only in the expanded dialog |
//! | `flatten` | The field is itself a `Params` struct; its parameters are included here |
//! | `no_persist` | Do not remember the last used value |

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Lit, LitStr, Meta, parse_macro_input};

#[proc_macro_derive(Params, attributes(param))]
pub fn derive_params(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Params can only be derived for structs",
        ));
    };
    let fields = match &data.fields {
        Fields::Named(named) => named.named.iter().collect::<Vec<_>>(),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                &data.fields,
                "Params needs named fields; each field name becomes a parameter name",
            ));
        }
    };

    let mut specs = Vec::new();
    let mut extractions = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().expect("named field");
        let name = ident.to_string();
        let name = name.strip_prefix("r#").unwrap_or(&name).to_string();
        let ty = &field.ty;

        let flatten = field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("param"))
            .any(|a| matches!(&a.meta, Meta::List(list) if list.tokens.to_string().trim() == "flatten"));
        if flatten {
            specs.push(quote! {
                specs.extend(<#ty as ::calipix_command::Params>::specs());
            });
            extractions.push(quote! {
                #ident: <#ty as ::calipix_command::Params>::from_args(args)?
            });
            continue;
        }

        let mut modifiers = Vec::new();
        let doc = doc_comment(&field.attrs);
        if !doc.is_empty() {
            modifiers.push(quote! { .doc(#doc) });
        }
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("param")) {
            attr.parse_nested_meta(|meta| {
                let key = meta
                    .path
                    .get_ident()
                    .map(|i| i.to_string())
                    .unwrap_or_default();
                match key.as_str() {
                    "label" => {
                        let v: LitStr = meta.value()?.parse()?;
                        modifiers.push(quote! { .label(#v) });
                    }
                    "default" => {
                        let v: Expr = meta.value()?.parse()?;
                        modifiers.push(quote! { .default_value(#v) });
                    }
                    "min" | "max" | "step" => {
                        let v: Expr = meta.value()?.parse()?;
                        let method = meta.path.get_ident().expect("checked above");
                        modifiers.push(quote! { .#method((#v) as f64) });
                    }
                    "unit" => {
                        let v: LitStr = meta.value()?.parse()?;
                        modifiers.push(quote! { .unit(#v) });
                    }
                    "ijm" => {
                        let v: LitStr = meta.value()?.parse()?;
                        modifiers.push(quote! { .ijm_key(#v) });
                    }
                    "position" => {
                        let v: LitStr = meta.value()?.parse()?;
                        let axis = match v.value().as_str() {
                            "channel" => quote! { Channel },
                            "slice" => quote! { Slice },
                            "frame" => quote! { Frame },
                            "stack" => quote! { Stack },
                            _ => {
                                return Err(syn::Error::new_spanned(
                                    &v,
                                    "expected \"channel\", \"slice\", \"frame\" or \"stack\"",
                                ));
                            }
                        };
                        modifiers.push(quote! {
                            .position(::calipix_command::PositionAxis::#axis)
                        });
                    }
                    "advanced" => modifiers.push(quote! { .advanced(true) }),
                    "no_persist" => modifiers.push(quote! { .persist(false) }),
                    "choices" => {
                        let content;
                        syn::parenthesized!(content in meta.input);
                        let choices = content.parse_terminated(
                            <LitStr as syn::parse::Parse>::parse,
                            syn::Token![,],
                        )?;
                        let choices = choices.iter();
                        modifiers.insert(
                            0,
                            quote! {
                                .with_type(::calipix_command::ParamType::Choice(
                                    ::std::vec![#(::std::string::String::from(#choices)),*]
                                ))
                            },
                        );
                    }
                    "path" => {
                        let v: LitStr = meta.value()?.parse()?;
                        let mode = match v.value().as_str() {
                            "open" => quote! { Open },
                            "save" => quote! { Save },
                            "dir" => quote! { Directory },
                            _ => {
                                return Err(syn::Error::new_spanned(
                                    &v,
                                    "expected \"open\", \"save\" or \"dir\"",
                                ));
                            }
                        };
                        modifiers.insert(
                            0,
                            quote! {
                                .with_type(::calipix_command::ParamType::Path(
                                    ::calipix_command::PathMode::#mode
                                ))
                            },
                        );
                    }
                    _ => {
                        return Err(meta.error(
                            "unknown key; expected one of: label, default, min, max, step, \
                             unit, choices, path, ijm, position, advanced, no_persist, flatten",
                        ));
                    }
                }
                Ok(())
            })?;
        }

        specs.push(quote! {
            specs.push(
                ::calipix_command::ParamSpec::new(
                    #name,
                    <#ty as ::calipix_command::ParamField>::param_type(),
                )
                .required(<#ty as ::calipix_command::ParamField>::REQUIRED)
                #(#modifiers)*
            );
        });
        extractions.push(quote! {
            #ident: <#ty as ::calipix_command::ParamField>::from_arg(args.get(#name), #name)?
        });
    }

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::calipix_command::Params for #ident #ty_generics #where_clause {
            fn specs() -> ::std::vec::Vec<::calipix_command::ParamSpec> {
                #[allow(unused_mut)]
                let mut specs = ::std::vec::Vec::new();
                #(#specs)*
                specs
            }

            fn from_args(args: &::calipix_command::Args) -> ::calipix_command::Result<Self> {
                ::std::result::Result::Ok(Self { #(#extractions),* })
            }
        }
    })
}

/// Joins a field's `///` lines into one paragraph.
fn doc_comment(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Str(s) => Some(s.value().trim().to_string()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
