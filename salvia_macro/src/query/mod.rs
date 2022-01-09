use proc_macro::TokenStream;

use syn::spanned::Spanned;
use syn::{Error, Item, ReturnType, Type};

mod parameter;
mod visitors;

use parameter::{ParameterError, Parameters};

pub fn query_impl(item: Item) -> Result<TokenStream, Error> {
    use quote::{quote, quote_spanned};
    use syn::{ItemFn, Signature};

    let ItemFn {
        attrs,
        vis: host_visibility,
        sig,
        block,
    } = match item {
        Item::Fn(item_fn) => item_fn,
        item => return Err(Error::new(item.span(), "#[query]: function is expected")),
    };

    let Signature {
        constness,
        asyncness,
        unsafety,
        abi,
        fn_token: _,
        ident: host_ident,
        mut generics,
        paren_token,
        inputs,
        variadic,
        output,
    } = sig;

    let output_type: Option<&Type> = match &output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => Some(ty),
    };

    // Some sanity checks.
    {
        let mut errs = vec![];

        if let Some(constness) = constness {
            errs.push(Error::new(
                constness.span(),
                "#[query]: cannot be applied to const functions",
            ))
        }

        if let Some(unsafety) = unsafety {
            errs.push(Error::new(
                unsafety.span(),
                "#[query]: cannot be applied to unsafe functions",
            ))
        }

        if let Some(abi) = abi {
            errs.push(Error::new(
                abi.span(),
                "#[query]: cannot be applied to external functions",
            ))
        }

        if let Some(variadic) = variadic {
            errs.push(Error::new(
                variadic.span(),
                "#[query]: cannot be applied to variadic functions",
            ))
        }

        // This provides slightly better error message than default.
        if asyncness.is_none() {
            errs.push(Error::new(
                host_ident.span(),
                "#[query]: can only be applied to async functions",
            ))
        }

        if let Some(output_type) = output_type {
            use syn::visit::Visit;
            use visitors::ImplTraitVisitor;

            let mut visitor: ImplTraitVisitor = Default::default();
            visitor.visit_type(output_type);

            let iter = visitor.0.into_iter()
                .map(|span| Error::new(span,
                                       "#[query]: `impl Trait` syntax is not supported, consider returning concrete type"));

            errs.extend(iter);
        }

        let err = errs.into_iter().reduce(|mut aggr, err| {
            aggr.combine(err);
            aggr
        });

        if let Some(err) = err {
            return Err(err);
        }
    }

    let where_clause = generics.where_clause.take();
    let generic_params = generics;

    let params = Parameters::from_inputs(inputs)
        .map_err(|err| match err {
            ParameterError::MissingContext => Error::new(paren_token.span, "#[query]: function should accept &::salvia::QueryContext as its last parameter"),
            ParameterError::UnexpectedSelf(span) => Error::new(span, "#[query]: unexpected `self` parameter in function"),
            ParameterError::Attribute(span) => Error::new(span, "#[query]: attributes on parameters are not currently supported"),
            ParameterError::PatternDestructuring(span) => Error::new(span, "#[query]: pattern destructuring in parameters is not supported; consider destructuring inside function body instead"),
            ParameterError::WildcardPattern(span) => Error::new(span, "#[query]: wildcard pattern is not supported, consider giving it a name starting with `_`, for ex. `_my_var`; even though your function doesn't need that parameter, query must capture and cache it to correctly function"),
            ParameterError::ImplTrait(span) => Error::new(span, "#[query]: `impl Trait` syntax is not supported, consider turning it into an explicit generic parameter")
        })?;

    // We cannot alias types as some types might be implicit generics.
    // Instead, just generate some shorthands and expand them everywhere.
    #[allow(non_snake_case)]
    let Input = {
        let rendered_receiver_type = params.receiver_type().map(|ty| quote! {#ty,});
        let common_types = params.common_types();

        quote! { (#rendered_receiver_type #(#common_types,)*) }
    };
    #[allow(non_snake_case)]
    let Output = match output_type {
        Some(ty) => quote! { #ty },
        None => quote! { () },
    };

    // Generate hosted function.
    //
    // Signature:
    //      F: FnOnce(Input, QueryContext) -> Ret + Clone + Send + Sync + 'static,
    //      Ret: Future<Output=(Value, QueryContext)> + Send + 'static,
    //
    // We cannot take context by ref because compiler blows up on lifetimes + closures + async,
    // so instead we pass it through.
    let tenant = {
        use syn::Index;

        let tenant_input_pats = params.common_args();
        let tenant_context_pat = params.context_arg();

        let common_types = params.common_types();
        let context_type = params.context_type();

        // Skip `self`: it is both captured by closure and provided as part of input.
        let start = if params.has_receiver() { 1 } else { 0 };
        let end = start + params.common_len();
        let index = (start..end).map(Index::from);

        quote! {
            let __tenant = move |input: #Input, __cx: ::salvia::QueryContext| async move {
                let val = {
                    let (#(#tenant_input_pats,)* #tenant_context_pat,): (#(#common_types,)* #context_type,) = (#(input.#index,)* &__cx,);

                    // Need to silence the warning in case user function never returns.
                    #[allow(unreachable_code)]
                    async move #block.await
                };

                (val, __cx)
            };
        }
    };

    let host = {
        let self_type = params.receiver_type();
        let context_type = params.context_type();
        let common_types = params.common_types();

        let host_self_arg = params.receiver_arg();
        let host_context_arg = params.context_ident();
        let host_common_args = params.common_idents();
        let common_idents = params.common_idents();

        // There doesn't seem to be any good way to interpolate comma only when option is `Some`,
        // so we just prerender relevant bits with comma.
        let rendered_host_self_arg = params.receiver.as_ref().map(|(arg, ty)| {
            use parameter::ReceiverIdent;

            match arg {
                ReceiverIdent::Receiver(receiver) => quote! { #receiver, },
                ReceiverIdent::PatIdent(pat) => quote! { #pat: #ty, },
            }
        });
        let rendered_host_internal_self = host_self_arg.map(|_| quote! {Clone::clone(&self),});

        let self_check = self_type.map(|ty| quote_spanned! {ty.span()=>
            let _ = ::salvia::macro_support::checks::StashableCheck::<#ty>(::std::marker::PhantomData);
            let _ = ::salvia::macro_support::checks::HashCheck::<#ty>(::std::marker::PhantomData);
        });

        let common_checks = params.common_types()
            .map(|ty| quote_spanned! {ty.span()=>
            let _ = ::salvia::macro_support::checks::StashableCheck::<#ty>(::std::marker::PhantomData);
        });

        let manager_checks = params.common_types()
            .map(|ty| quote_spanned! {ty.span()=>
            let _ = ::salvia::macro_support::checks::HashCheck::<#ty>(::std::marker::PhantomData);
        });

        let context_check = quote_spanned! {context_type.span()=>
            let _: ::std::marker::PhantomData::<&::salvia::QueryContext> = ::std::marker::PhantomData::<#context_type>;
        };

        let output_check = quote_spanned! {output_type.span()=>
            let _ = ::salvia::macro_support::checks::StashableCheck::<#Output>(::std::marker::PhantomData);
        };

        quote! {
            #(#attrs )*
            #host_visibility async fn #host_ident #generic_params(#rendered_host_self_arg #(#host_common_args: #common_types,)* #host_context_arg: &::salvia::QueryContext) #output #where_clause {
                #self_check
                #(#common_checks)*
                #(#manager_checks)*
                #context_check
                #output_check

                // Collect input before instantiating any local variables.
                let __input = (#rendered_host_internal_self #(#common_idents,)*);
                let __cx = #host_context_arg;

                #tenant

                ::salvia::node::query::executor(__tenant, __input, __cx).await.unwrap()
            }
        }
    };

    Ok(host.into())
}
