use proc_macro2::Span;
use syn::{FnArg, Ident, PatIdent, Receiver, Type};

pub enum ReceiverIdent {
    Receiver(Receiver),
    PatIdent(PatIdent),
}

pub struct Parameters {
    pub receiver: Option<(ReceiverIdent, Type)>,
    pub common: Vec<(PatIdent, Type)>,
    pub context: (PatIdent, Type),
}

pub enum ParameterError {
    MissingContext,
    UnexpectedSelf(Span),
    Attribute(Span),
    PatternDestructuring(Span),
    WildcardPattern(Span),
    ImplTrait(Span),
}

impl Parameters {
    pub fn from_inputs(iter: impl IntoIterator<Item = FnArg>) -> Result<Self, ParameterError> {
        use syn::spanned::Spanned;
        use syn::{Pat, PatType};

        fn process_pat(pat: PatType) -> Result<(PatIdent, Type), ParameterError> {
            use super::visitors::ImplTraitVisitor;
            use syn::visit::Visit;

            if let Some(attr) = pat.attrs.first() {
                return Err(ParameterError::Attribute(attr.span()));
            }

            let ty = *pat.ty;
            let mut visitor = ImplTraitVisitor::default();
            visitor.visit_type(&ty);

            if let Some(span) = visitor.0.into_iter().next() {
                return Err(ParameterError::ImplTrait(span));
            }

            let ident = match *pat.pat {
                Pat::Ident(pat) => {
                    if let Some(attr) = pat.attrs.first() {
                        return Err(ParameterError::Attribute(attr.span()));
                    }

                    if let Some(subpat) = &pat.subpat {
                        return Err(ParameterError::PatternDestructuring(subpat.1.span()));
                    }

                    pat
                }
                Pat::Wild(pat) => return Err(ParameterError::WildcardPattern(pat.span())),
                pat => return Err(ParameterError::PatternDestructuring(pat.span())),
            };

            Ok((ident, ty))
        }

        let mut iter = iter.into_iter();
        let (receiver, arg) = {
            let arg = iter.next().ok_or(ParameterError::MissingContext)?;

            match arg {
                FnArg::Receiver(receiver) => {
                    use syn::parse_quote;

                    let and_token = receiver.reference.as_ref().map(|r| r.0);
                    let lifetime = receiver.reference.as_ref().map(|r| r.1.as_ref()).flatten();
                    // Mutability is part of type only when it is a reference.
                    let mutability = and_token.and(receiver.mutability.as_ref());

                    // Make synthetic type for our own use.
                    let ty: Type = parse_quote! {
                        #and_token#lifetime #mutability Self
                    };

                    let receiver = (ReceiverIdent::Receiver(receiver), ty);
                    (Some(receiver), None)
                }
                FnArg::Typed(pat) => {
                    let (ident, ty) = process_pat(pat)?;

                    if ident.ident == "self" {
                        let receiver = (ReceiverIdent::PatIdent(ident), ty);
                        (Some(receiver), None)
                    } else {
                        let arg = (ident, ty);
                        (None, Some(arg))
                    }
                }
            }
        };

        let mut common: Vec<_> = arg
            .map(Ok)
            .into_iter()
            .chain(iter.map(|arg| match arg {
                FnArg::Receiver(_) => Err(ParameterError::UnexpectedSelf(arg.span())),
                FnArg::Typed(pat) => process_pat(pat),
            }))
            .collect::<Result<_, _>>()?;

        let context = common.pop().ok_or(ParameterError::MissingContext)?;

        let r = Parameters {
            receiver,
            common,
            context,
        };

        Ok(r)
    }

    pub fn has_receiver(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn common_len(&self) -> usize {
        self.common.len()
    }

    pub fn receiver_type(&self) -> Option<&Type> {
        self.receiver.as_ref().map(|t| &t.1)
    }

    pub fn context_type(&self) -> &Type {
        &self.context.1
    }

    pub fn common_types(&self) -> impl Iterator<Item = &Type> {
        self.common.iter().map(|t| &t.1)
    }

    pub fn receiver_arg(&self) -> Option<&ReceiverIdent> {
        self.receiver.as_ref().map(|t| &t.0)
    }

    pub fn context_arg(&self) -> &PatIdent {
        &self.context.0
    }

    pub fn common_args(&self) -> impl Iterator<Item = &PatIdent> {
        self.common.iter().map(|t| &t.0)
    }

    pub fn context_ident(&self) -> &Ident {
        &self.context.0.ident
    }

    pub fn common_idents(&self) -> impl Iterator<Item = &Ident> + Clone {
        self.common.iter().map(|t| &t.0.ident)
    }
}
