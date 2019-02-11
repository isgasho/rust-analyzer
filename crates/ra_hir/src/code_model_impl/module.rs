use ra_db::FileId;
use ra_syntax::{ast, SyntaxNode, TreeArc};

use crate::{
    Module, ModuleSource, Problem,
    Name,
    module_tree::ModuleId,
    impl_block::ImplId,
    nameres::{lower::ImportId},
    HirDatabase, PersistentHirDatabase,
};

impl Module {
    fn with_module_id(&self, module_id: ModuleId) -> Module {
        Module { module_id, krate: self.krate }
    }

    pub(crate) fn name_impl(&self, db: &dyn HirDatabase) -> Option<Name> {
        let module_tree = db.module_tree(self.krate);
        let link = self.module_id.parent_link(&module_tree)?;
        Some(link.name(&module_tree).clone())
    }

    pub(crate) fn definition_source_impl(
        &self,
        db: &dyn PersistentHirDatabase,
    ) -> (FileId, ModuleSource) {
        let module_tree = db.module_tree(self.krate);
        let file_id = self.module_id.file_id(&module_tree);
        let decl_id = self.module_id.decl_id(&module_tree);
        let module_source = ModuleSource::new(db, file_id, decl_id);
        let file_id = file_id.as_original_file();
        (file_id, module_source)
    }

    pub(crate) fn declaration_source_impl(
        &self,
        db: &dyn HirDatabase,
    ) -> Option<(FileId, TreeArc<ast::Module>)> {
        let module_tree = db.module_tree(self.krate);
        let link = self.module_id.parent_link(&module_tree)?;
        let file_id = link.owner(&module_tree).file_id(&module_tree).as_original_file();
        let src = link.source(&module_tree, db.as_ref());
        Some((file_id, src))
    }

    pub(crate) fn import_source_impl(
        &self,
        db: &dyn HirDatabase,
        import: ImportId,
    ) -> TreeArc<ast::PathSegment> {
        let source_map = db.lower_module_source_map(*self);
        let (_, source) = self.definition_source(db.as_ref());
        source_map.get(&source, import)
    }

    pub(crate) fn impl_source_impl(
        &self,
        db: &dyn HirDatabase,
        impl_id: ImplId,
    ) -> TreeArc<ast::ImplBlock> {
        let source_map = db.impls_in_module_source_map(*self);
        let (_, source) = self.definition_source(db.as_ref());
        source_map.get(&source, impl_id)
    }

    pub(crate) fn crate_root_impl(&self, db: &dyn PersistentHirDatabase) -> Module {
        let module_tree = db.module_tree(self.krate);
        let module_id = self.module_id.crate_root(&module_tree);
        self.with_module_id(module_id)
    }

    /// Finds a child module with the specified name.
    pub(crate) fn child_impl(&self, db: &dyn HirDatabase, name: &Name) -> Option<Module> {
        let module_tree = db.module_tree(self.krate);
        let child_id = self.module_id.child(&module_tree, name)?;
        Some(self.with_module_id(child_id))
    }

    /// Iterates over all child modules.
    pub(crate) fn children_impl(
        &self,
        db: &dyn PersistentHirDatabase,
    ) -> impl Iterator<Item = Module> {
        let module_tree = db.module_tree(self.krate);
        let children = self
            .module_id
            .children(&module_tree)
            .map(|(_, module_id)| self.with_module_id(module_id))
            .collect::<Vec<_>>();
        children.into_iter()
    }

    pub(crate) fn parent_impl(&self, db: &dyn PersistentHirDatabase) -> Option<Module> {
        let module_tree = db.module_tree(self.krate);
        let parent_id = self.module_id.parent(&module_tree)?;
        Some(self.with_module_id(parent_id))
    }

    pub(crate) fn problems_impl(
        &self,
        db: &dyn HirDatabase,
    ) -> Vec<(TreeArc<SyntaxNode>, Problem)> {
        let module_tree = db.module_tree(self.krate);
        self.module_id.problems(&module_tree, db)
    }
}
