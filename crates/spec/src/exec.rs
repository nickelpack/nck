use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use nck_hashing::{StableHash, StableHasherExt};

use crate::{Action, LinkFlags};

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionAction {
    Exec {
        path: PathBuf,
        args: Vec<OsString>,
        env: Vec<(OsString, OsString)>,
        work_dir: PathBuf,
    },
    Link {
        from: PathBuf,
        to: PathBuf,
        flags: LinkFlags,
    },
}

impl StableHash for ExecutionAction {
    fn update<H: nck_hashing::StableHasher>(&self, h: &mut H) {
        match self {
            ExecutionAction::Exec {
                path,
                args,
                env,
                work_dir,
            } => h
                .update_hash(1u8)
                .update_hash(path)
                .update_iter(args.iter())
                .update_iter(env.iter())
                .update_hash(work_dir),
            ExecutionAction::Link { from, to, flags } => h
                .update_hash(2u8)
                .update_hash(from)
                .update_hash(to)
                .update_hash(flags.bits()),
        };
    }
}

pub struct ExecutionIterator<'a> {
    pub(crate) spec: &'a [Action],
    pub(crate) rest: bool,
    pub(crate) env: BTreeMap<OsString, OsString>,
    pub(crate) work_dir: PathBuf,
}

impl<'a> Iterator for ExecutionIterator<'a> {
    type Item = ExecutionAction;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest && !self.spec.is_empty() {
                self.spec = &self.spec[1..];
            } else {
                self.rest = true;
            }

            match self.spec.first()? {
                crate::Action::Link(link) => {
                    return Some(ExecutionAction::Link {
                        from: link.from.clone(),
                        to: link.to.clone(),
                        flags: link.flags,
                    })
                }
                crate::Action::Exec(exec) => {
                    return Some(ExecutionAction::Exec {
                        path: exec.path.clone(),
                        args: exec.args.clone(),
                        work_dir: self.work_dir.clone(),
                        env: self
                            .env
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                    });
                }
                crate::Action::Set(set) => {
                    match set.value.as_ref() {
                        Some(url) => self.env.insert(set.name.to_os_string(), url.to_os_string()),
                        None => self.env.remove(&set.name),
                    };
                }
                crate::Action::WorkDir(w) => self.work_dir = w.path.clone(),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{ExecutionAction, Spec};
    use pretty_assertions::assert_eq;

    #[test]
    fn exec() -> anyhow::Result<()> {
        let spec = Spec::builder("test")
            .add_output("out")
            .link(
                "/var/nck/store/files/abc123",
                "/bin/test1",
                Some(crate::LinkFlags::EXECUTABLE),
            )
            .link("/var/nck/store/files/foo", "/bin/test", None)
            .exec("/bin/test", vec!["hello".into(), "world".into()])
            .work_dir("/test")
            .set("value", Some("value2"))
            .set("test", Some("value"))
            .exec("/bin/foo", vec!["hello".into(), "world".into()])
            .unset("value")
            .work_dir("/test/foo")
            .exec("/bin/sh", vec!["-c".into(), "test".into()])
            .build()?;

        let v: Vec<ExecutionAction> = spec.iterate_execution().collect();

        assert_eq!(
            &[
                ExecutionAction::Link {
                    from: "/var/nck/store/files/abc123".into(),
                    to: "/bin/test1".into(),
                    flags: crate::LinkFlags::EXECUTABLE
                },
                ExecutionAction::Link {
                    from: "/var/nck/store/files/foo".into(),
                    to: "/bin/test".into(),
                    flags: crate::LinkFlags::empty()
                },
                ExecutionAction::Exec {
                    path: "/bin/test".into(),
                    args: vec!["hello".into(), "world".into()],
                    env: vec![],
                    work_dir: "/".into()
                },
                ExecutionAction::Exec {
                    path: "/bin/foo".into(),
                    args: vec!["hello".into(), "world".into()],
                    env: vec![
                        ("test".into(), "value".into()),
                        ("value".into(), "value2".into())
                    ],
                    work_dir: "/test".into()
                },
                ExecutionAction::Exec {
                    path: "/bin/sh".into(),
                    args: vec!["-c".into(), "test".into()],
                    env: vec![("test".into(), "value".into()),],
                    work_dir: "/test/foo".into()
                },
            ],
            &v[..]
        );

        Ok(())
    }
}
