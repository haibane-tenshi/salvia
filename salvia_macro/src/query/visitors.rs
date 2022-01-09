use proc_macro2::Span;
use syn::visit::Visit;
use syn::TypeImplTrait;

#[derive(Default)]
pub struct ImplTraitVisitor(pub Vec<Span>);

impl<'ast> Visit<'ast> for ImplTraitVisitor {
    fn visit_type_impl_trait(&mut self, node: &'ast TypeImplTrait) {
        use syn::spanned::Spanned;

        self.0.push(node.span())
    }
}
