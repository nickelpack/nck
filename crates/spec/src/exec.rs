use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use nck_hashing::SupportedHash;
use url::Url;

use crate::Action;

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionAction {
    Fetch {
        source: Option<Url>,
        integrity: SupportedHash,
    },
    Exec {
        path: PathBuf,
        args: Vec<OsString>,
        env: Vec<(OsString, OsString)>,
        work_dir: PathBuf,
    },
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
                crate::Action::Fetch(fetch) => {
                    return Some(ExecutionAction::Fetch {
                        source: fetch.source.clone(),
                        integrity: fetch.integrity,
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
            .exec("/bin/test", vec!["hello".into(), "world".into()])
            .fetch(
                Some("https://www.example.com".parse()?),
                "blake3-awoddymtijbosmenspc5ml64fvsmshe7fuecdyitferqilkudhla".parse()?,
            )
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
                ExecutionAction::Exec {
                    path: "/bin/test".into(),
                    args: vec!["hello".into(), "world".into()],
                    env: vec![],
                    work_dir: "/".into()
                },
                ExecutionAction::Fetch {
                    source: Some("https://www.example.com".parse()?),
                    integrity: "blake3-awoddymtijbosmenspc5ml64fvsmshe7fuecdyitferqilkudhla"
                        .parse()?
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
