use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

/// `derive_config_schema` function.
/// `derive_config_schema` 函数.
#[proc_macro_derive(ConfigSchema)]
pub fn derive_config_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = input.ident;
    let expanded = quote! {
        impl ::cheetah_sdk::config::ConfigSchema for #ident
        where
            #ident: ::core::default::Default
                + ::serde::Serialize
                + for<'de> ::serde::Deserialize<'de>
                + Send
                + Sync
                + 'static,
        {
            fn schema_name() -> &'static str {
                stringify!(#ident)
            }

            fn default_json() -> ::serde_json::Value {
                ::serde_json::to_value(<#ident as ::core::default::Default>::default())
                    .unwrap_or(::serde_json::Value::Null)
            }
        }
    };
    expanded.into()
}
