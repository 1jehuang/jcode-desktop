use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{AttrStyle, Attribute};

pub(crate) fn emit(name: Ident, attrs: Vec<Attribute>, body: TokenStream) -> TokenStream {
    // ItemFn stores both styles together, but prints inner attributes inside the braces.
    let outer = attrs.iter().filter(|a| matches!(a.style, AttrStyle::Outer));
    let inner = attrs
        .iter()
        .filter(|a| matches!(a.style, AttrStyle::Inner(_)));
    quote! { #[test] #(#outer)* fn #name() { #(#inner)* #body } }
}
