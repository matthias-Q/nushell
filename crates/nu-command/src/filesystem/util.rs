use dialoguer::Input;
use nu_engine::{command_prelude::*, get_eval_expression};
use nu_protocol::{ast::Expr, FromValue, NuGlob};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Resource {
    pub at: usize,
    pub location: PathBuf,
}

impl Resource {}

pub fn try_interaction(
    interactive: bool,
    prompt: String,
) -> (Result<Option<bool>, Box<dyn Error>>, bool) {
    let interaction = if interactive {
        match get_interactive_confirmation(prompt) {
            Ok(i) => Ok(Some(i)),
            Err(e) => Err(e),
        }
    } else {
        Ok(None)
    };

    let confirmed = match interaction {
        Ok(maybe_input) => maybe_input.unwrap_or(false),
        Err(_) => false,
    };

    (interaction, confirmed)
}

fn get_interactive_confirmation(prompt: String) -> Result<bool, Box<dyn Error>> {
    let input = Input::new()
        .with_prompt(prompt)
        .validate_with(|c_input: &String| -> Result<(), String> {
            if c_input.len() == 1
                && (c_input == "y" || c_input == "Y" || c_input == "n" || c_input == "N")
            {
                Ok(())
            } else if c_input.len() > 1 {
                Err("Enter only one letter (Y/N)".to_string())
            } else {
                Err("Input not valid".to_string())
            }
        })
        .default("Y/N".into())
        .interact_text()?;

    if input == "y" || input == "Y" {
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Return `Some(true)` if the last change time of the `src` old than the `dst`,
/// otherwisie return `Some(false)`. Return `None` if the `src` or `dst` doesn't exist.
#[allow(dead_code)]
pub fn is_older(src: &Path, dst: &Path) -> Option<bool> {
    if !dst.exists() || !src.exists() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let src_ctime = std::fs::metadata(src)
            .map(|m| m.ctime())
            .unwrap_or(i64::MIN);
        let dst_ctime = std::fs::metadata(dst)
            .map(|m| m.ctime())
            .unwrap_or(i64::MAX);
        Some(src_ctime <= dst_ctime)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let src_ctime = std::fs::metadata(src)
            .map(|m| m.last_write_time())
            .unwrap_or(u64::MIN);
        let dst_ctime = std::fs::metadata(dst)
            .map(|m| m.last_write_time())
            .unwrap_or(u64::MAX);
        Some(src_ctime <= dst_ctime)
    }
}

#[cfg(unix)]
pub mod users {
    use nix::unistd::{Gid, Group, Uid, User};

    pub fn get_user_by_uid(uid: Uid) -> Option<User> {
        User::from_uid(uid).ok().flatten()
    }

    pub fn get_group_by_gid(gid: Gid) -> Option<Group> {
        Group::from_gid(gid).ok().flatten()
    }

    pub fn get_current_uid() -> Uid {
        Uid::current()
    }

    pub fn get_current_gid() -> Gid {
        Gid::current()
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "android")))]
    pub fn get_current_username() -> Option<String> {
        get_user_by_uid(get_current_uid()).map(|user| user.name)
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "android"))]
    pub fn current_user_groups() -> Option<Vec<Gid>> {
        if let Ok(mut groups) = nix::unistd::getgroups() {
            groups.sort_unstable_by_key(|id| id.as_raw());
            groups.dedup();
            Some(groups)
        } else {
            None
        }
    }

    /// Returns groups for a provided user name and primary group id.
    ///
    /// # libc functions used
    ///
    /// - [`getgrouplist`](https://docs.rs/libc/*/libc/fn.getgrouplist.html)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use users::get_user_groups;
    ///
    /// for group in get_user_groups("stevedore", 1001).expect("Error looking up groups") {
    ///     println!("User is a member of group #{group}");
    /// }
    /// ```
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "android")))]
    pub fn get_user_groups(username: &str, gid: Gid) -> Option<Vec<Gid>> {
        use nix::libc::{c_int, gid_t};
        use std::ffi::CString;

        // MacOS uses i32 instead of gid_t in getgrouplist for unknown reasons
        #[cfg(target_os = "macos")]
        let mut buff: Vec<i32> = vec![0; 1024];
        #[cfg(not(target_os = "macos"))]
        let mut buff: Vec<gid_t> = vec![0; 1024];

        let name = CString::new(username).ok()?;

        let mut count = buff.len() as c_int;

        // MacOS uses i32 instead of gid_t in getgrouplist for unknown reasons
        // SAFETY:
        // int getgrouplist(const char *user, gid_t group, gid_t *groups, int *ngroups);
        //
        // `name` is valid CStr to be `const char*` for `user`
        // every valid value will be accepted for `group`
        // The capacity for `*groups` is passed in as `*ngroups` which is the buffer max length/capacity (as we initialize with 0)
        // Following reads from `*groups`/`buff` will only happen after `buff.truncate(*ngroups)`
        #[cfg(target_os = "macos")]
        let res = unsafe {
            nix::libc::getgrouplist(
                name.as_ptr(),
                gid.as_raw() as i32,
                buff.as_mut_ptr(),
                &mut count,
            )
        };

        #[cfg(not(target_os = "macos"))]
        let res = unsafe {
            nix::libc::getgrouplist(name.as_ptr(), gid.as_raw(), buff.as_mut_ptr(), &mut count)
        };

        if res < 0 {
            None
        } else {
            buff.truncate(count as usize);
            buff.sort_unstable();
            buff.dedup();
            // allow trivial cast: on macos i is i32, on linux it's already gid_t
            #[allow(trivial_numeric_casts)]
            Some(
                buff.into_iter()
                    .map(|id| Gid::from_raw(id as gid_t))
                    .filter_map(get_group_by_gid)
                    .map(|group| group.gid)
                    .collect(),
            )
        }
    }
}

/// Get rest arguments from given `call`, starts with `starting_pos`.
///
/// It's similar to `call.rest`, except that it always returns NuGlob.  And if input argument has
/// Type::Glob, the NuGlob is unquoted, which means it's required to expand.
pub fn get_rest_for_glob_pattern(
    engine_state: &EngineState,
    stack: &mut Stack,
    call: &Call,
    starting_pos: usize,
) -> Result<Vec<Spanned<NuGlob>>, ShellError> {
    let mut output = vec![];
    let eval_expression = get_eval_expression(engine_state);

    for result in call.rest_iter_flattened(starting_pos, |expr| {
        let result = eval_expression(engine_state, stack, expr);
        match result {
            Err(e) => Err(e),
            Ok(result) => {
                let span = result.span();
                // convert from string to quoted string if expr is a variable
                // or string interpolation
                match result {
                    Value::String { val, .. }
                        if matches!(
                            &expr.expr,
                            Expr::FullCellPath(_) | Expr::StringInterpolation(_)
                        ) =>
                    {
                        // should not expand if given input type is not glob.
                        Ok(Value::glob(val, expr.ty != Type::Glob, span))
                    }
                    other => Ok(other),
                }
            }
        }
    })? {
        output.push(FromValue::from_value(result)?);
    }

    Ok(output)
}

/// Get optional arguments from given `call` with position `pos`.
///
/// It's similar to `call.opt`, except that it always returns NuGlob.
pub fn opt_for_glob_pattern(
    engine_state: &EngineState,
    stack: &mut Stack,
    call: &Call,
    pos: usize,
) -> Result<Option<Spanned<NuGlob>>, ShellError> {
    if let Some(expr) = call.positional_nth(pos) {
        let eval_expression = get_eval_expression(engine_state);
        let result = eval_expression(engine_state, stack, expr)?;
        let result_span = result.span();
        let result = match result {
            Value::String { val, .. }
                if matches!(
                    &expr.expr,
                    Expr::FullCellPath(_) | Expr::StringInterpolation(_)
                ) =>
            {
                // should quote if given input type is not glob.
                Value::glob(val, expr.ty != Type::Glob, result_span)
            }
            other => other,
        };
        FromValue::from_value(result).map(Some)
    } else {
        Ok(None)
    }
}
