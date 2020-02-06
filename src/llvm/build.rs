use bumpalo::collections::Vec;
use bumpalo::Bump;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::BasicTypeEnum;
use inkwell::values::BasicValueEnum::{self, *};
use inkwell::values::{FunctionValue, IntValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};
use inlinable_string::InlinableString;

use crate::collections::ImMap;
use crate::llvm::convert::{
    content_to_basic_type, get_fn_type, layout_to_basic_type, type_from_var,
};
use crate::mono::expr::{Expr, Proc, Procs};
use crate::subs::{Subs, Variable};

/// This is for Inkwell's FunctionValue::verify - we want to know the verification
/// output in debug builds, but we don't want it to print to stdout in release builds!
#[cfg(debug_assertions)]
const PRINT_FN_VERIFICATION_OUTPUT: bool = true;

#[cfg(not(debug_assertions))]
const PRINT_FN_VERIFICATION_OUTPUT: bool = false;

type Scope<'ctx> = ImMap<InlinableString, (Variable, PointerValue<'ctx>)>;

pub struct Env<'a, 'ctx, 'env> {
    pub arena: &'a Bump,
    pub context: &'ctx Context,
    pub builder: &'env Builder<'ctx>,
    pub module: &'ctx Module<'ctx>,
    pub subs: Subs,
}

pub fn build_expr<'a, 'ctx, 'env>(
    env: &Env<'a, 'ctx, 'env>,
    scope: &Scope<'ctx>,
    parent: FunctionValue<'ctx>,
    expr: &Expr<'a>,
    procs: &Procs<'a>,
) -> BasicValueEnum<'ctx> {
    use crate::mono::expr::Expr::*;

    match expr {
        Int(num) => env.context.i64_type().const_int(*num as u64, false).into(),
        Float(num) => env.context.f64_type().const_float(*num).into(),
        Cond {
            cond_lhs,
            cond_rhs,
            pass,
            fail,
            ret_var,
            ..
        } => {
            let cond = Branch2 {
                cond_lhs,
                cond_rhs,
                pass,
                fail,
                ret_var: *ret_var,
            };

            build_branch2(env, scope, parent, cond, procs)
        }
        Branches { .. } => {
            panic!("TODO build_branches(env, scope, parent, cond_lhs, branches, procs)");
        }
        Switch {
            cond,
            branches,
            default_branch,
            ret_var,
            cond_var,
        } => {
            let ret_type = type_from_var(*ret_var, &env.subs, env.context);
            let switch_args = SwitchArgs {
                cond_var: *cond_var,
                cond_expr: cond,
                branches,
                default_branch,
                ret_type,
            };

            build_switch(env, scope, parent, switch_args, procs)
        }
        Store(ref stores, ref ret) => {
            let mut scope = im_rc::HashMap::clone(scope);
            let subs = &env.subs;
            let context = &env.context;

            for (name, var, expr) in stores.iter() {
                let content = subs.get_without_compacting(*var).content;
                let val = build_expr(env, &scope, parent, &expr, procs);
                let expr_bt =
                    content_to_basic_type(&content, subs, context).unwrap_or_else(|err| {
                        panic!(
                            "Error converting symbol {:?} to basic type: {:?} - scope was: {:?}",
                            name, err, scope
                        )
                    });
                let alloca = create_entry_block_alloca(env, parent, expr_bt, &name);

                env.builder.build_store(alloca, val);

                // Make a new scope which includes the binding we just encountered.
                // This should be done *after* compiling the bound expr, since any
                // recursive (in the LetRec sense) bindings should already have
                // been extracted as procedures. Nothing in here should need to
                // access itself!
                scope = im_rc::HashMap::clone(&scope);

                scope.insert(name.clone(), (*var, alloca));
            }

            build_expr(env, &scope, parent, ret, procs)
        }
        CallByName(ref name, ref args) => {
            // TODO try one of these alternative strategies (preferably the latter):
            //
            // 1. use SIMD string comparison to compare these strings faster
            // 2. pre-register Bool.or using module.add_function, and see if LLVM inlines it
            // 3. intern all these strings
            if name == "Bool.or" {
                panic!("TODO create a phi node for ||");
            } else if name == "Bool.and" {
                panic!("TODO create a phi node for &&");
            } else {
                let mut arg_vals: Vec<BasicValueEnum> =
                    Vec::with_capacity_in(args.len(), env.arena);

                for arg in args.iter() {
                    arg_vals.push(build_expr(env, scope, parent, arg, procs));
                }

                let fn_val = env
                    .module
                    .get_function(name)
                    .unwrap_or_else(|| panic!("Unrecognized function: {:?}", name));

                let call = env.builder.build_call(fn_val, arg_vals.as_slice(), "tmp");

                call.try_as_basic_value().left().unwrap_or_else(|| {
                    panic!("LLVM error: Invalid call by name for name {:?}", name)
                })
            }
        }
        FunctionPointer(ref fn_name) => {
            let ptr = env
                .module
                .get_function(fn_name)
                .unwrap_or_else(|| {
                    panic!("Could not get pointer to unknown function {:?}", fn_name)
                })
                .as_global_value()
                .as_pointer_value();

            BasicValueEnum::PointerValue(ptr)
        }
        CallByPointer(ref sub_expr, ref args, _var) => {
            let mut arg_vals: Vec<BasicValueEnum> = Vec::with_capacity_in(args.len(), env.arena);

            for arg in args.iter() {
                arg_vals.push(build_expr(env, scope, parent, arg, procs));
            }

            let call = match build_expr(env, scope, parent, sub_expr, procs) {
                BasicValueEnum::PointerValue(ptr) => {
                    env.builder.build_call(ptr, arg_vals.as_slice(), "tmp")
                }
                non_ptr => {
                    panic!(
                        "Tried to call by pointer, but encountered a non-pointer: {:?}",
                        non_ptr
                    );
                }
            };

            call.try_as_basic_value()
                .left()
                .unwrap_or_else(|| panic!("LLVM error: Invalid call by pointer."))
        }

        Load(name) => match scope.get(name) {
            Some((_, ptr)) => env.builder.build_load(*ptr, name),
            None => panic!("Could not find a var for {:?} in scope {:?}", name, scope),
        },
        _ => {
            panic!("I don't yet know how to LLVM build {:?}", expr);
        }
    }
}

struct Branch2<'a> {
    cond_lhs: &'a Expr<'a>,
    cond_rhs: &'a Expr<'a>,
    pass: &'a Expr<'a>,
    fail: &'a Expr<'a>,
    ret_var: Variable,
}

fn build_branch2<'a, 'ctx, 'env>(
    env: &Env<'a, 'ctx, 'env>,
    scope: &Scope<'ctx>,
    parent: FunctionValue<'ctx>,
    cond: Branch2<'a>,
    procs: &Procs<'a>,
) -> BasicValueEnum<'ctx> {
    let builder = env.builder;
    let context = env.context;
    let subs = &env.subs;

    let content = subs.get_without_compacting(cond.ret_var).content;
    let ret_type = content_to_basic_type(&content, subs, context).unwrap_or_else(|err| {
        panic!(
            "Error converting cond branch ret_type content {:?} to basic type: {:?}",
            cond.pass, err
        )
    });

    let lhs = build_expr(env, scope, parent, cond.cond_lhs, procs);
    let rhs = build_expr(env, scope, parent, cond.cond_rhs, procs);

    match (lhs, rhs) {
        (FloatValue(lhs_float), FloatValue(rhs_float)) => {
            let comparison =
                builder.build_float_compare(FloatPredicate::OEQ, lhs_float, rhs_float, "cond");

            build_phi2(
                env, scope, parent, comparison, cond.pass, cond.fail, ret_type, procs,
            )
        }

        (IntValue(lhs_int), IntValue(rhs_int)) => {
            let comparison = builder.build_int_compare(IntPredicate::EQ, lhs_int, rhs_int, "cond");

            build_phi2(
                env, scope, parent, comparison, cond.pass, cond.fail, ret_type, procs,
            )
        }
        _ => panic!(
            "Tried to make a branch out of incompatible conditions: lhs = {:?} and rhs = {:?}",
            cond.cond_lhs, cond.cond_rhs
        ),
    }
}

struct SwitchArgs<'a, 'ctx> {
    pub cond_expr: &'a Expr<'a>,
    pub cond_var: Variable,
    pub branches: &'a [(u64, Expr<'a>)],
    pub default_branch: &'a Expr<'a>,
    pub ret_type: BasicTypeEnum<'ctx>,
}

fn build_switch<'a, 'ctx, 'env>(
    env: &Env<'a, 'ctx, 'env>,
    scope: &Scope<'ctx>,
    parent: FunctionValue<'ctx>,
    switch_args: SwitchArgs<'a, 'ctx>,
    procs: &Procs<'a>,
) -> BasicValueEnum<'ctx> {
    let arena = env.arena;
    let builder = env.builder;
    let context = env.context;
    let SwitchArgs {
        branches,
        cond_expr,
        default_branch,
        ret_type,
        ..
    } = switch_args;

    let cont_block = context.append_basic_block(parent, "cont");

    // Build the condition
    let cond = build_expr(env, scope, parent, cond_expr, procs).into_int_value();

    // Build the cases
    let mut incoming = Vec::with_capacity_in(branches.len(), arena);
    let mut cases = Vec::with_capacity_in(branches.len(), arena);

    for (int, _) in branches.iter() {
        let int_val = context.i64_type().const_int(*int as u64, false);
        let block = context.append_basic_block(parent, format!("branch{}", int).as_str());

        cases.push((int_val, &*arena.alloc(block)));
    }

    let default_block = context.append_basic_block(parent, "default");

    builder.build_switch(cond, &default_block, &cases);

    for ((_, branch_expr), (_, block)) in branches.iter().zip(cases) {
        builder.position_at_end(&block);

        let branch_val = build_expr(env, scope, parent, branch_expr, procs);

        builder.build_unconditional_branch(&cont_block);

        incoming.push((branch_val, block));
    }

    // The block for the conditional's default branch.
    builder.position_at_end(&default_block);

    let default_val = build_expr(env, scope, parent, default_branch, procs);

    builder.build_unconditional_branch(&cont_block);

    incoming.push((default_val, &default_block));

    // emit merge block
    builder.position_at_end(&cont_block);

    let phi = builder.build_phi(ret_type, "branch");

    for (branch_val, block) in incoming {
        phi.add_incoming(&[(&Into::<BasicValueEnum>::into(branch_val), block)]);
    }

    phi.as_basic_value()
}

// TODO trim down these arguments
#[allow(clippy::too_many_arguments)]
fn build_phi2<'a, 'ctx, 'env>(
    env: &Env<'a, 'ctx, 'env>,
    scope: &Scope<'ctx>,
    parent: FunctionValue<'ctx>,
    comparison: IntValue<'ctx>,
    pass: &'a Expr<'a>,
    fail: &'a Expr<'a>,
    ret_type: BasicTypeEnum<'ctx>,
    procs: &Procs<'a>,
) -> BasicValueEnum<'ctx> {
    let builder = env.builder;
    let context = env.context;

    // build blocks
    let then_block = context.append_basic_block(parent, "then");
    let else_block = context.append_basic_block(parent, "else");
    let cont_block = context.append_basic_block(parent, "branchcont");

    builder.build_conditional_branch(comparison, &then_block, &else_block);

    // build then block
    builder.position_at_end(&then_block);
    let then_val = build_expr(env, scope, parent, pass, procs);
    builder.build_unconditional_branch(&cont_block);

    let then_block = builder.get_insert_block().unwrap();

    // build else block
    builder.position_at_end(&else_block);
    let else_val = build_expr(env, scope, parent, fail, procs);
    builder.build_unconditional_branch(&cont_block);

    let else_block = builder.get_insert_block().unwrap();

    // emit merge block
    builder.position_at_end(&cont_block);

    let phi = builder.build_phi(ret_type, "branch");

    phi.add_incoming(&[
        (&Into::<BasicValueEnum>::into(then_val), &then_block),
        (&Into::<BasicValueEnum>::into(else_val), &else_block),
    ]);

    phi.as_basic_value()
}

/// TODO could this be added to Inkwell itself as a method on BasicValueEnum?
fn set_name(bv_enum: BasicValueEnum<'_>, name: &str) {
    match bv_enum {
        ArrayValue(val) => val.set_name(name),
        IntValue(val) => val.set_name(name),
        FloatValue(val) => val.set_name(name),
        PointerValue(val) => val.set_name(name),
        StructValue(val) => val.set_name(name),
        VectorValue(val) => val.set_name(name),
    }
}

/// Creates a new stack allocation instruction in the entry block of the function.
pub fn create_entry_block_alloca<'a, 'ctx>(
    env: &Env<'a, 'ctx, '_>,
    parent: FunctionValue<'_>,
    basic_type: BasicTypeEnum<'ctx>,
    name: &str,
) -> PointerValue<'ctx> {
    let builder = env.context.create_builder();
    let entry = parent.get_first_basic_block().unwrap();

    match entry.get_first_instruction() {
        Some(first_instr) => builder.position_before(&first_instr),
        None => builder.position_at_end(&entry),
    }

    builder.build_alloca(basic_type, name)
}

pub fn build_proc<'a, 'ctx, 'env>(
    env: &Env<'a, 'ctx, 'env>,
    name: InlinableString,
    proc: Proc<'a>,
    procs: &Procs<'a>,
) -> FunctionValue<'ctx> {
    let args = proc.args;
    let arena = env.arena;
    let subs = &env.subs;
    let context = &env.context;
    let ret_content = subs.get_without_compacting(proc.ret_var).content;
    // TODO this content_to_basic_type is duplicated when building this Proc
    let ret_type = content_to_basic_type(&ret_content, subs, context).unwrap_or_else(|err| {
        panic!(
            "Error converting function return value content to basic type: {:?}",
            err
        )
    });
    let mut arg_basic_types = Vec::with_capacity_in(args.len(), arena);
    let mut arg_names = Vec::new_in(arena);

    for (layout, name, _var) in args.iter() {
        let arg_type = layout_to_basic_type(&layout, subs, env.context);

        arg_basic_types.push(arg_type);
        arg_names.push(name);
    }

    let fn_type = get_fn_type(&ret_type, &arg_basic_types);

    let fn_val = env
        .module
        .add_function(&name, fn_type, Some(Linkage::Private));

    // Add a basic block for the entry point
    let entry = context.append_basic_block(fn_val, "entry");
    let builder = env.builder;

    builder.position_at_end(&entry);

    let mut scope = ImMap::default();

    // Add args to scope
    for ((arg_val, arg_type), (_, arg_name, var)) in
        fn_val.get_param_iter().zip(arg_basic_types).zip(args)
    {
        set_name(arg_val, arg_name);

        let alloca = create_entry_block_alloca(env, fn_val, arg_type, arg_name);

        builder.build_store(alloca, arg_val);

        scope.insert(arg_name.clone(), (*var, alloca));
    }

    let body = build_expr(env, &scope, fn_val, &proc.body, procs);

    builder.build_return(Some(&body));

    fn_val
}

pub fn verify_fn(fn_val: FunctionValue<'_>) {
    if !fn_val.verify(PRINT_FN_VERIFICATION_OUTPUT) {
        unsafe {
            fn_val.delete();
        }

        panic!("Invalid generated fn_val.")
    }
}