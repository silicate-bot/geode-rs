use broma_rs::{AccessModifier, FunctionBindField, FunctionType};

fn mangle_ident(s: &str) -> String {
    if s.contains("::") {
        let mut result = String::new();
        let mut parts: Vec<&str> = s.split("::").collect();
        parts.reverse();
        for part in parts {
            result.push_str(part);
            result.push('@');
        }
        result
    } else {
        format!("{s}@")
    }
}

fn access_token(proto: &broma_rs::MemberFunctionProto) -> char {
    if proto.is_virtual {
        match proto.access {
            AccessModifier::Private => 'E',
            AccessModifier::Protected => 'M',
            AccessModifier::Public => 'U',
        }
    } else {
        match proto.access {
            AccessModifier::Private => 'A',
            AccessModifier::Protected => 'I',
            AccessModifier::Public => 'Q',
        }
    }
}

fn split_qualified(name: &str) -> (&str, Option<&str>) {
    if let Some(index) = name.rfind("::") {
        (&name[index + 2..], Some(&name[..index]))
    } else {
        (name, None)
    }
}

fn strip_const(mut ty: &str) -> (&str, bool) {
    let mut is_const = false;
    loop {
        if let Some(rest) = ty.strip_prefix("const ") {
            ty = rest.trim();
            is_const = true;
            continue;
        }
        if let Some(rest) = ty.strip_suffix(" const") {
            ty = rest.trim();
            is_const = true;
            continue;
        }
        break;
    }
    (ty, is_const)
}

fn primitive_code(ty: &str) -> Option<&'static str> {
    match ty {
        "void" => Some("X"),
        "bool" => Some("_N"),
        "char" => Some("D"),
        "unsigned char" => Some("E"),
        "short" => Some("F"),
        "unsigned short" => Some("G"),
        "int" => Some("H"),
        "unsigned int" => Some("I"),
        "long" => Some("J"),
        "unsigned long" => Some("K"),
        "float" => Some("M"),
        "double" => Some("N"),
        _ => None,
    }
}

fn encode_class_path(current_class: &str, ty: &str) -> String {
    let (current_short, current_namespace) = split_qualified(current_class);
    let (short, namespace) = split_qualified(ty);

    if namespace == current_namespace {
        if short == current_short {
            "12@".to_string()
        } else {
            format!("{short}@2@")
        }
    } else {
        format!("{}@", mangle_ident(ty))
    }
}

fn encode_enum_type(current_class: &str, ty: &str) -> String {
    let (current_namespace_short, current_namespace) = split_qualified(current_class);
    let _ = current_namespace_short;
    let (short, namespace) = split_qualified(ty);

    if namespace == current_namespace {
        format!("W4{short}@2@")
    } else {
        format!("W4{}@", mangle_ident(ty))
    }
}

fn encode_pointer_type(current_class: &str, ty: &str) -> Option<String> {
    let inner = ty.strip_suffix('*')?.trim();
    let (inner, is_const) = strip_const(inner);

    if let Some(code) = primitive_code(inner) {
        let cv = if is_const { 'B' } else { 'A' };
        return Some(format!("PE{cv}{code}"));
    }

    let path = encode_class_path(current_class, inner);
    let cv = if is_const { 'B' } else { 'A' };
    Some(format!("PE{cv}V{path}"))
}

fn encode_value_type(current_class: &str, ty: &str, allow_enum: bool) -> Option<String> {
    let (ty, is_const) = strip_const(ty);
    if is_const {
        return None;
    }

    if let Some(pointer) = encode_pointer_type(current_class, ty) {
        return Some(pointer);
    }

    if let Some(code) = primitive_code(ty) {
        return Some(code.to_string());
    }

    if allow_enum {
        return Some(encode_enum_type(current_class, ty));
    }

    None
}

fn encode_arg_type(current_class: &str, ty: &str, seen: &mut Vec<String>) -> Option<String> {
    let encoded = encode_value_type(current_class, ty, true)?;
    if encoded == "_N"
        && let Some(index) = seen.iter().position(|seen_ty| seen_ty == &encoded)
        && index < 10
    {
        return Some(index.to_string());
    }
    seen.push(encoded.clone());
    Some(encoded)
}

fn encode_return_type(current_class: &str, ty: &str) -> Option<String> {
    encode_value_type(current_class, ty, false)
}

pub fn generate_windows_symbol(class_name: &str, func: &FunctionBindField) -> Option<String> {
    let decl = &func.prototype;
    let access = access_token(decl);

    let mut symbol = match decl.fn_type {
        FunctionType::Constructor => format!("??0{}@{access}EAA", mangle_ident(class_name)),
        FunctionType::Destructor => format!("??1{}@{access}EAA", mangle_ident(class_name)),
        FunctionType::Normal => {
            if decl.is_static {
                format!("?{}@{}@SA", decl.name, mangle_ident(class_name))
            } else {
                let constness = if decl.is_const { 'B' } else { 'A' };
                format!(
                    "?{}@{}@{access}E{constness}A",
                    decl.name,
                    mangle_ident(class_name)
                )
            }
        }
    };

    if let FunctionType::Normal = decl.fn_type {
        symbol.push_str(&encode_return_type(class_name, &decl.ret.name)?);
    }

    if decl.args.is_empty() {
        symbol.push_str("XZ");
        return Some(symbol);
    }

    let mut seen = Vec::new();
    for arg in &decl.args {
        symbol.push_str(&encode_arg_type(class_name, &arg.ty.name, &mut seen)?);
    }

    symbol.push_str("@Z");
    Some(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use broma_rs::{Arg, MemberFunctionProto, Type};

    fn arg(name: &str, ty: &str) -> Arg {
        Arg {
            name: name.into(),
            ty: Type::new(ty),
        }
    }

    #[test]
    fn mangles_noarg_instance_method() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "drawScene".into(),
                ret: Type::new("void"),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCDirector", &func).as_deref(),
            Some("?drawScene@CCDirector@cocos2d@@QEAAXXZ")
        );
    }

    #[test]
    fn mangles_dispatch_insert_text() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "dispatchInsertText".into(),
                ret: Type::new("void"),
                args: vec![
                    arg("", "char const*"),
                    arg("", "int"),
                    arg("", "cocos2d::enumKeyCodes"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCIMEDispatcher", &func).as_deref(),
            Some("?dispatchInsertText@CCIMEDispatcher@cocos2d@@QEAAXPEBDHW4enumKeyCodes@2@@Z")
        );
    }

    #[test]
    fn mangles_dispatch_keyboard_msg() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "dispatchKeyboardMSG".into(),
                ret: Type::new("bool"),
                args: vec![
                    arg("", "cocos2d::enumKeyCodes"),
                    arg("", "bool"),
                    arg("", "bool"),
                    arg("", "double"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCKeyboardDispatcher", &func).as_deref(),
            Some(
                "?dispatchKeyboardMSG@CCKeyboardDispatcher@cocos2d@@QEAA_NW4enumKeyCodes@2@_N1N@Z"
            )
        );
    }

    #[test]
    fn mangles_dispatch_scroll_msg() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "dispatchScrollMSG".into(),
                ret: Type::new("bool"),
                args: vec![arg("", "float"), arg("", "float")],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCMouseDispatcher", &func).as_deref(),
            Some("?dispatchScrollMSG@CCMouseDispatcher@cocos2d@@QEAA_NMM@Z")
        );
    }

    #[test]
    fn mangles_touches() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "touches".into(),
                ret: Type::new("void"),
                args: vec![
                    arg("", "cocos2d::CCSet*"),
                    arg("", "cocos2d::CCEvent*"),
                    arg("", "unsigned int"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCTouchDispatcher", &func).as_deref(),
            Some("?touches@CCTouchDispatcher@cocos2d@@QEAAXPEAVCCSet@2@PEAVCCEvent@2@I@Z")
        );
    }

    #[test]
    fn mangles_same_class_pointer_return() {
        let func = FunctionBindField {
            prototype: MemberFunctionProto {
                name: "sharedDirector".into(),
                ret: Type::new("cocos2d::CCDirector*"),
                is_static: true,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            generate_windows_symbol("cocos2d::CCDirector", &func).as_deref(),
            Some("?sharedDirector@CCDirector@cocos2d@@SAPEAV12@XZ")
        );
    }
}
