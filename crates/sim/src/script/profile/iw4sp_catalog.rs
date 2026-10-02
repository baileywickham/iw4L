use super::catalog::{Builtin, Namespace::*, Owner::*};

pub(super) const IW4SP: &[Builtin] = &[
    Builtin::new(Function, "getallnodes", Script, false),
    Builtin::new(Function, "getnode", Script, false),
    Builtin::new(Function, "getnodearray", Script, false),
];
