use proc_macro::TokenStream;

use syn::{DeriveInput, Error};

pub fn hash_impl(data: DeriveInput) -> Result<TokenStream, Error> {
    use quote::quote;

    let ident = data.ident;
    let mut generic_params = data.generics;
    let where_clause = generic_params.where_clause.take();

    let impls = quote! {
        impl #generic_params #ident #generic_params #where_clause {
            pub async fn set<Value>(self, value: Value, cx: &::salvia::InputContext)
                where Self: ::salvia::Input<Value>, Value: ::salvia::Stashable
            {
                ::salvia::node::input::set_executor(self, value, cx).await.unwrap()
            }

            pub async fn get<Value>(self, cx: &::salvia::QueryContext) -> Value
                where Self: ::salvia::Input<Value>, Value: ::salvia::Stashable
            {
                ::salvia::node::input::get_executor(self, cx).await.unwrap()
            }
        }
    };

    Ok(impls.into())
}
