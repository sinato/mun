use super::{
    adt,
    dispatch_table::{DispatchTable, DispatchTableBuilder},
    intrinsics,
    type_table::{TypeTable, TypeTableBuilder},
};
use crate::value::{IrTypeContext, IrValueContext};
use crate::IrDatabase;
use hir::ModuleDef;
use inkwell::{module::Module, types::PointerType, values::UnnamedAddress, AddressSpace};
use std::{collections::BTreeMap, sync::Arc};

/// The IR generated for a group of files. It is used to generate IR for all of the group's files
/// and the resulting `Assembly`'s symbols.
#[derive(Debug, PartialEq, Eq)]
pub struct FileGroupIR {
    /// The LLVM module that contains the IR
    pub(crate) llvm_module: Module,
    /// The dispatch table
    pub(crate) dispatch_table: DispatchTable,
    /// The type table
    pub(crate) type_table: TypeTable,
    /// The allocator handle, if it exists
    pub(crate) allocator_handle_type: Option<PointerType>,
}

/// Generates IR that is shared among the group's files.
/// TODO: Currently, a group always consists of a single file. Need to add support for multiple
/// files using something like `FileGroupId`.
pub(crate) fn ir_query(db: &impl IrDatabase, file_id: hir::FileId) -> Arc<FileGroupIR> {
    let llvm_module = db.context().create_module("group_name");

    // Use a `BTreeMap` to guarantee deterministically ordered output.
    let mut intrinsics_map = BTreeMap::new();
    let mut needs_alloc = false;

    // println!("group ir query start!!!!!!!!!!!!!!");
    // Collect all intrinsic functions, wrapper function, and generate struct declarations.
    for def in db.module_data(file_id).definitions() {
        match def {
            ModuleDef::Function(f) if !f.is_extern(db) => {
                intrinsics::collect_fn_body(
                    db,
                    &mut intrinsics_map,
                    &mut needs_alloc,
                    &f.body(db),
                    &f.infer(db),
                );

                let fn_sig = f.ty(db).callable_sig(db).unwrap();
                if !f.data(db).visibility().is_private() && !fn_sig.marshallable(db) {
                    intrinsics::collect_wrapper_body(db, &mut intrinsics_map, &mut needs_alloc);
                }
            }
            ModuleDef::Function(_) => (), // TODO: Extern types?
            ModuleDef::Struct(s) => {
                adt::gen_struct_decl(db, *s);
            }
            ModuleDef::BuiltinType(_) => (),
        }
    }

    // println!("group ir query end1!!!!!!!!!!!!!!");
    // Collect all exposed functions' bodies.
    let mut dispatch_table_builder = DispatchTableBuilder::new(db, &llvm_module, &intrinsics_map);
    for def in db.module_data(file_id).definitions() {
        if let ModuleDef::Function(f) = def {
            if !f.data(db).visibility().is_private() && !f.is_extern(db) {
                let body = f.body(db);
                let infer = f.infer(db);
                dispatch_table_builder.collect_body(&body, &infer);
            }
        }
    }
    // println!("group ir query end2!!!!!!!!!!!!!!");

    let dispatch_table = dispatch_table_builder.build();

    let struct_types = db.type_to_struct_mapping();

    let type_context = IrTypeContext {
        context: &db.context(),
        target_data: &db.target_data(),
        struct_types: struct_types.as_ref(),
    };
    let value_context = IrValueContext {
        type_context: &type_context,
        context: &db.context(),
        module: &llvm_module,
    };
    let mut type_table_builder =
        TypeTableBuilder::new(db, &value_context, intrinsics_map.keys(), &dispatch_table);

    // Collect all used types
    for def in db.module_data(file_id).definitions() {
        match def {
            ModuleDef::Struct(s) => {
                type_table_builder.collect_struct(*s);
            }
            ModuleDef::Function(f) => {
                type_table_builder.collect_fn(*f);
            }
            ModuleDef::BuiltinType(_) => (),
        }
    }

    let type_table = type_table_builder.build();

    // Create the allocator handle global value
    let allocator_handle_type = if needs_alloc {
        let allocator_handle_type = db.context().i8_type().ptr_type(AddressSpace::Generic);
        let global = llvm_module.add_global(allocator_handle_type, None, "allocatorHandle");
        global.set_initializer(&allocator_handle_type.const_null());
        global.set_unnamed_address(UnnamedAddress::Global);
        Some(allocator_handle_type)
    } else {
        None
    };

    Arc::new(FileGroupIR {
        llvm_module,
        dispatch_table,
        type_table,
        allocator_handle_type,
    })
}
