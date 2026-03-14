/// Build a named state predicate from a Rust boolean expression.
#[macro_export]
macro_rules! pred {
    ($name:ident ($state:pat_param) => $expr:expr $(,)?) => {
        $crate::BoolExpr::pure_call(stringify!($name), |$state| $expr)
    };
}

/// Build a named transition predicate from a Rust boolean expression.
#[macro_export]
macro_rules! step {
    ($name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?) => {
        $crate::StepExpr::pure_call(stringify!($name), |$prev, $action, $next| $expr)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nirvash_state_constraint_pred {
    ($name:ident ($state:pat_param) => $expr:expr $(,)?) => {
        $crate::pred!($name($state) => $expr)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nirvash_action_constraint_pred {
    ($name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?) => {
        $crate::step!($name($prev, $action, $next) => $expr)
    };
}

/// Build an LTL formula using Rust boolean operators and temporal keywords.
#[macro_export]
macro_rules! ltl {
    (true) => {
        $crate::Ltl::truth()
    };
    (false) => {
        $crate::Ltl::falsity()
    };
    (pred!($name:ident ($state:pat_param) => $expr:expr $(,)?)) => {
        $crate::Ltl::pred($crate::pred!($name($state) => $expr))
    };
    (step!($name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?)) => {
        $crate::Ltl::step($crate::step!($name($prev, $action, $next) => $expr))
    };
    (enabled(step!($name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?))) => {
        $crate::Ltl::enabled($crate::step!($name($prev, $action, $next) => $expr))
    };
    ((! $($inner:tt)+)) => {
        $crate::Ltl::negate($crate::ltl!($($inner)+))
    };
    (always($($inner:tt)+)) => {
        $crate::Ltl::always($crate::ltl!($($inner)+))
    };
    (eventually($($inner:tt)+)) => {
        $crate::Ltl::eventually($crate::ltl!($($inner)+))
    };
    (next($($inner:tt)+)) => {
        $crate::Ltl::next($crate::ltl!($($inner)+))
    };
    (($lhs:tt && $rhs:tt)) => {
        $crate::Ltl::and($crate::ltl!($lhs), $crate::ltl!($rhs))
    };
    (($lhs:tt || $rhs:tt)) => {
        $crate::Ltl::or($crate::ltl!($lhs), $crate::ltl!($rhs))
    };
    (($lhs:tt => $rhs:tt)) => {
        $crate::Ltl::implies($crate::ltl!($lhs), $crate::ltl!($rhs))
    };
    (until($lhs:tt, $rhs:tt)) => {
        $crate::Ltl::until($crate::ltl!($lhs), $crate::ltl!($rhs))
    };
    (leads_to($lhs:tt, $rhs:tt)) => {
        $crate::Ltl::leads_to($crate::ltl!($lhs), $crate::ltl!($rhs))
    };
    (($($inner:tt)+)) => {
        $crate::ltl!($($inner)+)
    };
}

/// Declare and register an invariant with the proc-macro registry.
#[macro_export]
macro_rules! invariant {
    ($spec:path, $name:ident ($state:pat_param) => $expr:expr $(,)?) => {
        #[::nirvash_macros::invariant($spec)]
        fn $name() -> $crate::BoolExpr<<$spec as ::nirvash_lower::FrontendSpec>::State> {
            $crate::pred!($name($state) => $expr)
        }
    };
}

/// Declare and register a state constraint.
#[macro_export]
macro_rules! state_constraint {
    ($spec:path, $name:ident ($state:pat_param) => $expr:expr $(,)?) => {
        #[::nirvash_macros::state_constraint($spec)]
        fn $name() -> $crate::BoolExpr<<$spec as ::nirvash_lower::FrontendSpec>::State> {
            $crate::__nirvash_state_constraint_pred!($name($state) => $expr)
        }
    };
}

/// Declare and register an action constraint.
#[macro_export]
macro_rules! action_constraint {
    ($spec:path, $name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?) => {
        #[::nirvash_macros::action_constraint($spec)]
        fn $name() -> $crate::StepExpr<
            <$spec as ::nirvash_lower::FrontendSpec>::State,
            <$spec as ::nirvash_lower::FrontendSpec>::Action,
        > {
            $crate::__nirvash_action_constraint_pred!($name($prev, $action, $next) => $expr)
        }
    };
}

/// Declare and register an LTL property.
#[macro_export]
macro_rules! property {
    ($spec:path, $name:ident => $($formula:tt)+) => {
        #[::nirvash_macros::property($spec)]
        fn $name() -> $crate::Ltl<
            <$spec as ::nirvash_lower::FrontendSpec>::State,
            <$spec as ::nirvash_lower::FrontendSpec>::Action,
        > {
            $crate::ltl!($($formula)+)
        }
    };
}

/// Declare and register a weak or strong fairness assumption.
#[macro_export]
macro_rules! fairness {
    (weak $spec:path, $name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?) => {
        #[::nirvash_macros::fairness($spec)]
        fn $name() -> $crate::Fairness<
            <$spec as ::nirvash_lower::FrontendSpec>::State,
            <$spec as ::nirvash_lower::FrontendSpec>::Action,
        > {
            $crate::Fairness::weak($crate::step!($name($prev, $action, $next) => $expr))
        }
    };
    (strong $spec:path, $name:ident ($prev:pat_param, $action:pat_param, $next:pat_param) => $expr:expr $(,)?) => {
        #[::nirvash_macros::fairness($spec)]
        fn $name() -> $crate::Fairness<
            <$spec as ::nirvash_lower::FrontendSpec>::State,
            <$spec as ::nirvash_lower::FrontendSpec>::Action,
        > {
            $crate::Fairness::strong($crate::step!($name($prev, $action, $next) => $expr))
        }
    };
}

/// Register pure helper keys that future symbolic encoders may accept.
#[macro_export]
macro_rules! register_symbolic_pure_helpers {
    ($($key:expr),+ $(,)?) => {
        $(
            $crate::inventory::submit! {
                $crate::registry::RegisteredSymbolicPureHelper {
                    key: $key,
                }
            }
        )+
    };
}

/// Register effect keys that future symbolic encoders may accept.
#[macro_export]
macro_rules! register_symbolic_effects {
    ($($key:expr),+ $(,)?) => {
        $(
            $crate::inventory::submit! {
                $crate::registry::RegisteredSymbolicEffect {
                    key: $key,
                }
            }
        )+
    };
}

/// Implement a generated finite-domain companion trait with less boilerplate.
///
/// This is a manual fallback for `#[derive(FiniteModelDomain)] #[finite_model_domain(custom)]` cases
/// where bounds/filter attributes are not expressive enough.
#[macro_export]
macro_rules! finite_model_domain_spec {
    (
        $trait:ident for $ty:ty,
        representatives = $representatives:expr
        $(, filter($filter_self:ident) => $filter:expr)?
        $(, invariant($invariant_self:ident) => $invariant:expr)?
        $(,)?
    ) => {
        impl $trait for $ty {
            fn finite_domain() -> $crate::BoundedDomain<Self> {
                let __nirvash_domain = $crate::into_bounded_domain($representatives);
                $(
                    let __nirvash_domain = __nirvash_domain.filter(|$filter_self| $filter);
                )?
                __nirvash_domain
            }

            fn value_invariant(&self) -> bool {
                true $(
                    && {
                        let $invariant_self = self;
                        $invariant
                    }
                )?
            }
        }

        $crate::inventory::submit! {
            $crate::registry::RegisteredFiniteDomainSeed {
                value_type_id: ::std::any::TypeId::of::<$ty>,
                values: || {
                    <$ty as $crate::FiniteModelDomain>::finite_domain()
                        .into_vec()
                        .into_iter()
                        .map(|value| Box::new(value) as Box<dyn ::std::any::Any>)
                        .collect()
                },
            }
        }
    };
}

/// Implement symbolic encoding metadata for `#[derive(SymbolicEncoding)] #[symbolic_encoding(custom)]` states.
#[macro_export]
macro_rules! symbolic_state_spec {
    (
        for $ty:ty {
            $($field:ident : $field_ty:ty),+ $(,)?
        }
    ) => {
        const _: () = {
            impl ::nirvash_lower::SymbolicEncoding for $ty {
                fn symbolic_sort() -> $crate::SymbolicSort {
                    $crate::SymbolicSort::composite::<Self>(vec![
                        $(
                            $crate::SymbolicSortField::new(
                                stringify!($field),
                                <$field_ty as ::nirvash_lower::SymbolicEncoding>::symbolic_sort(),
                            ),
                        )+
                    ])
                }

                fn symbolic_state_schema() -> ::core::option::Option<$crate::SymbolicStateSchema<Self>> {
                    let mut __nirvash_fields = ::std::vec::Vec::new();
                    $(
                        __nirvash_fields.extend($crate::symbolic_state_fields::<Self, $field_ty, _, _>(
                            stringify!($field),
                            |state: &Self| &state.$field,
                            |state: &mut Self, value: $field_ty| {
                                state.$field = value;
                            },
                        ));
                    )+
                    ::core::option::Option::Some($crate::SymbolicStateSchema::new(__nirvash_fields, || Self {
                        $(
                            $field: $crate::symbolic_seed_value::<$field_ty>(),
                        )+
                    }))
                }
            }

            fn __nirvash_symbolic_state_type_id() -> ::std::any::TypeId {
                ::std::any::TypeId::of::<$ty>()
            }

            fn __nirvash_build_symbolic_state_schema() -> ::std::boxed::Box<dyn ::std::any::Any> {
                ::std::boxed::Box::new(
                    <$ty as ::nirvash_lower::SymbolicEncoding>::symbolic_state_schema()
                        .expect("symbolic state schema should be available")
                )
            }

            $crate::inventory::submit! {
                $crate::registry::RegisteredSymbolicStateSchema {
                    state_type_id: __nirvash_symbolic_state_type_id,
                    build: __nirvash_build_symbolic_state_schema,
                }
            }
        };
    };
}
