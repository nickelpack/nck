mod bytestring;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::InvalidSpec;

use bytestring::*;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename = "dependency")]
struct Dependency {
    integrity: String,
    output: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "spec")]
pub struct Spec {
    name: String,
    outputs: BTreeSet<String>,
    #[serde(default)]
    actions: Vec<super::Action>,
    #[serde(default)]
    dependencies: BTreeMap<String, BTreeSet<Dependency>>,
    #[serde(default)]
    files: BTreeSet<String>,
}

impl From<super::Spec> for Spec {
    fn from(value: super::Spec) -> Self {
        let mut dependencies: BTreeMap<String, BTreeSet<Dependency>> = BTreeMap::new();
        let mut files = BTreeSet::new();

        for dep in value.dependencies {
            match dep {
                crate::Dependency::Package(package) => match dependencies.entry(package.name) {
                    std::collections::btree_map::Entry::Vacant(e) => {
                        e.insert(
                            [Dependency {
                                integrity: package.integrity.to_string(),
                                output: package.output,
                            }]
                            .into_iter()
                            .collect(),
                        );
                    }
                    std::collections::btree_map::Entry::Occupied(mut e) => {
                        e.get_mut().insert(Dependency {
                            integrity: package.integrity.to_string(),
                            output: package.output,
                        });
                    }
                },
                crate::Dependency::File(f) => {
                    files.insert(f.integrity.to_string());
                }
            }
        }

        Self {
            name: value.name,
            outputs: value.outputs.iter().cloned().collect(),
            actions: value.actions.clone(),
            dependencies,
            files,
        }
    }
}

impl TryFrom<Spec> for super::Spec {
    type Error = InvalidSpec;

    fn try_from(value: Spec) -> Result<Self, Self::Error> {
        let mut dependencies = BTreeSet::new();
        for val in value.dependencies {
            for out in val.1 {
                dependencies.insert(super::Dependency::Package(super::PackageDependency {
                    name: val.0.clone(),
                    output: out.output,
                    integrity: out
                        .integrity
                        .parse()
                        .map_err(|_| InvalidSpec::InvalidHash(out.integrity))?,
                }));
            }
        }

        for val in value.files {
            dependencies.insert(super::Dependency::File(super::FileDependency {
                integrity: val.parse().map_err(|_| InvalidSpec::InvalidHash(val))?,
            }));
        }

        super::Spec::new(
            value.name,
            value.outputs.into_iter().collect(),
            value.actions,
            dependencies.into_iter(),
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "exec")]
pub struct Exec {
    path: ByteString,
    #[serde(default)]
    args: Vec<ByteString>,
}

impl From<super::Exec> for Exec {
    fn from(value: super::Exec) -> Self {
        Self {
            path: value.path.into(),
            args: value.args.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Exec> for super::Exec {
    fn from(value: Exec) -> Self {
        Self {
            path: value.path.into(),
            args: value.args.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "set")]
pub struct Set {
    name: ByteString,
    value: Option<ByteString>,
}

impl From<super::Set> for Set {
    fn from(value: super::Set) -> Self {
        Self {
            name: value.name.into(),
            value: value.value.map(Into::into),
        }
    }
}

impl TryFrom<Set> for super::Set {
    type Error = super::InvalidEnvironmentVariableName;

    fn try_from(value: Set) -> Result<Self, Self::Error> {
        super::InvalidEnvironmentVariableName::validate(value.name.value())?;
        Ok(Self {
            name: value.name.into(),
            value: value.value.map(Into::into),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "work_dir")]
pub struct WorkDir {
    path: ByteString,
}

impl From<super::WorkDir> for WorkDir {
    fn from(value: super::WorkDir) -> Self {
        Self {
            path: value.path.into(),
        }
    }
}

impl From<WorkDir> for super::WorkDir {
    fn from(value: WorkDir) -> Self {
        Self {
            path: value.path.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "link")]
pub struct Link {
    from: ByteString,
    to: ByteString,
    #[serde(default = "default_executable")]
    executable: bool,
}

fn default_executable() -> bool {
    true
}

impl From<super::Link> for Link {
    fn from(value: super::Link) -> Self {
        Self {
            from: value.from.into(),
            to: value.to.into(),
            executable: value.flags.contains(super::LinkFlags::EXECUTABLE),
        }
    }
}

impl From<Link> for super::Link {
    fn from(value: Link) -> Self {
        let mut flags = super::LinkFlags::empty();
        if value.executable {
            flags |= super::LinkFlags::EXECUTABLE;
        }

        Self {
            from: value.from.into(),
            to: value.to.into(),
            flags,
        }
    }
}

#[cfg(test)]
mod test {
    use std::ffi::OsString;

    use pretty_assertions::assert_eq;
    use serde_test::{assert_tokens, Token};

    use crate::Spec;

    // #[test]
    // fn ser_json() -> anyhow::Result<()> {
    //     let spec = Spec::builder("test")
    //         .add_output("testa")
    //         .exec("/test", vec![OsString::from("foo"), OsString::from("bar")])
    //         .build()?;
    //     let s = serde_json::to_string_pretty(&spec)?;
    //     assert_eq!("{}", s.as_str());
    //     Ok(())
    // }

    #[test]
    fn ser_de_spec_actions() -> anyhow::Result<()> {
        let spec = Spec::builder("test")
            .add_output("testa")
            .set("env1", Some("env2"))
            .link("/foo", "/bar", None)
            .link("/foo", "/baz", Some(crate::LinkFlags::EXECUTABLE))
            .exec("/test", vec![OsString::from("foo"), OsString::from("bar")])
            .build()?;
        assert_tokens(
            &spec,
            &[
                Token::Struct {
                    name: "spec",
                    len: 5,
                },
                Token::Str("name"),
                Token::Str("test"),
                Token::Str("outputs"),
                Token::Seq { len: Some(1) },
                Token::Str("testa"),
                Token::SeqEnd,
                Token::Str("actions"),
                Token::Seq { len: Some(4) },
                Token::Struct {
                    name: "set",
                    len: 3,
                },
                Token::Str("action"),
                Token::Str("set"),
                Token::Str("name"),
                Token::Str("env1"),
                Token::Str("value"),
                Token::Some,
                Token::Str("env2"),
                Token::StructEnd,
                Token::Struct {
                    name: "link",
                    len: 4,
                },
                Token::Str("action"),
                Token::Str("link"),
                Token::Str("from"),
                Token::Str("/foo"),
                Token::Str("to"),
                Token::Str("/bar"),
                Token::Str("executable"),
                Token::Bool(false),
                Token::StructEnd,
                Token::Struct {
                    name: "link",
                    len: 4,
                },
                Token::Str("action"),
                Token::Str("link"),
                Token::Str("from"),
                Token::Str("/foo"),
                Token::Str("to"),
                Token::Str("/baz"),
                Token::Str("executable"),
                Token::Bool(true),
                Token::StructEnd,
                Token::Struct {
                    name: "exec",
                    len: 3,
                },
                Token::Str("action"),
                Token::Str("exec"),
                Token::Str("path"),
                Token::Str("/test"),
                Token::Str("args"),
                Token::Seq { len: Some(2) },
                Token::Str("foo"),
                Token::Str("bar"),
                Token::SeqEnd,
                Token::StructEnd,
                Token::SeqEnd,
                Token::Str("dependencies"),
                Token::Map { len: Some(0) },
                Token::MapEnd,
                Token::Str("files"),
                Token::Seq { len: Some(0) },
                Token::SeqEnd,
                Token::StructEnd,
            ],
        );
        Ok(())
    }

    #[test]
    fn ser_de_spec_outputs() -> anyhow::Result<()> {
        let spec = Spec::builder("test")
            .add_output("testa")
            .add_output("testb")
            .exec("/test", Vec::new())
            .build()?;
        assert_tokens(
            &spec,
            &[
                Token::Struct {
                    name: "spec",
                    len: 5,
                },
                Token::Str("name"),
                Token::Str("test"),
                Token::Str("outputs"),
                Token::Seq { len: Some(2) },
                Token::Str("testa"),
                Token::Str("testb"),
                Token::SeqEnd,
                Token::Str("actions"),
                Token::Seq { len: Some(1) },
                Token::Struct {
                    name: "exec",
                    len: 3,
                },
                Token::Str("action"),
                Token::Str("exec"),
                Token::Str("path"),
                Token::Str("/test"),
                Token::Str("args"),
                Token::Seq { len: Some(0) },
                Token::SeqEnd,
                Token::StructEnd,
                Token::SeqEnd,
                Token::Str("dependencies"),
                Token::Map { len: Some(0) },
                Token::MapEnd,
                Token::Str("files"),
                Token::Seq { len: Some(0) },
                Token::SeqEnd,
                Token::StructEnd,
            ],
        );
        Ok(())
    }

    #[test]
    fn ser_de_spec_dependencies() -> anyhow::Result<()> {
        let spec = Spec::builder("test")
            .add_output("testa")
            .package(
                "foo-1.0",
                "out",
                nck_hashing::SupportedHash::Blake3(*b"12345678901234567890123456789012"),
            )
            .package(
                "foo-1.0",
                "dev",
                nck_hashing::SupportedHash::Blake3(*b"12345678901234567890123456789013"),
            )
            .file(nck_hashing::SupportedHash::Blake3(
                *b"abcdefghijklmnopqrstuvwxyz012345",
            ))
            .exec("/test", Vec::new())
            .build()?;
        assert_tokens(
            &spec,
            &[
                Token::Struct {
                    name: "spec",
                    len: 5,
                },
                Token::Str("name"),
                Token::Str("test"),
                Token::Str("outputs"),
                Token::Seq { len: Some(1) },
                Token::Str("testa"),
                Token::SeqEnd,
                Token::Str("actions"),
                Token::Seq { len: Some(1) },
                Token::Struct {
                    name: "exec",
                    len: 3,
                },
                Token::Str("action"),
                Token::Str("exec"),
                Token::Str("path"),
                Token::Str("/test"),
                Token::Str("args"),
                Token::Seq { len: Some(0) },
                Token::SeqEnd,
                Token::StructEnd,
                Token::SeqEnd,
                Token::Str("dependencies"),
                Token::Map { len: Some(1) },
                Token::Str("foo-1.0"),
                Token::Seq { len: Some(2) },
                Token::Struct {
                    name: "dependency",
                    len: 2,
                },
                Token::Str("integrity"),
                Token::Str("blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgeza"),
                Token::Str("output"),
                Token::Str("out"),
                Token::StructEnd,
                Token::Struct {
                    name: "dependency",
                    len: 2,
                },
                Token::Str("integrity"),
                Token::Str("blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgezq"),
                Token::Str("output"),
                Token::Str("dev"),
                Token::StructEnd,
                Token::SeqEnd,
                Token::MapEnd,
                Token::Str("files"),
                Token::Seq { len: Some(1) },
                Token::Str("blake3-mfrggzdfmztwq2lknnwg23tpobyxe43uov3ho6dzpiydcmrtgq2q"),
                Token::SeqEnd,
                Token::StructEnd,
            ],
        );
        Ok(())
    }

    #[test]
    fn de_spec_outputs_bad() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo"
            outputs = [ "test", "test 1" ]
            actions = [
                { action="exec", path="/test" }
            ]
        "##,
        )
        .unwrap_err();
        assert_eq!("invalid output name 'test 1'", value.message());
        Ok(())
    }

    #[test]
    fn de_spec_name_bad() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo 1"
            outputs = [ "test" ]
            actions = [
                { action="exec", path="/test" }
            ]
        "##,
        )
        .unwrap_err();
        assert_eq!("invalid package name 'foo 1'", value.message());
        Ok(())
    }

    #[test]
    fn de_spec_set_bad() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo"
            outputs = [ "test" ]
            actions = [
                { action="set", name="FOO=", value = "Value" },
                { action="exec", path="/test" }
            ]
        "##,
        )
        .unwrap_err();
        assert_eq!("invalid environment variable name", value.message());
        Ok(())
    }

    #[test]
    fn de_spec_actions_bad() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo"
            outputs = [ "b", "test", "a", "a" ]
            actions = [ ]
        "##,
        )
        .unwrap_err();
        assert_eq!(
            "the final command in the spec is not an exec",
            value.message()
        );
        Ok(())
    }

    #[test]
    fn de_spec_actions() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo"
            outputs = [ "b", "test", "a", "a" ]

            actions = [
                { action = "work_dir", path = "/foo" },
                { action = "exec", path = "/bar", args = [ "c", "a", "b" ] }
            ]
        "##,
        )?;
        assert_eq!(
            Spec::builder("foo")
                .add_output("a")
                .add_output("b")
                .add_output("test")
                .work_dir("/foo")
                .exec(
                    "/bar",
                    vec![
                        OsString::from("c"),
                        OsString::from("a"),
                        OsString::from("b")
                    ]
                )
                .build()?,
            value
        );
        Ok(())
    }

    #[test]
    fn de_spec_deps() -> anyhow::Result<()> {
        let value = toml::from_str::<Spec>(
            r##"
            name = "foo"

            outputs = [ "default" ]

            files = [
                "blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgfsa"
            ]

            actions = [
                { action = "exec", path = "/bar" }
            ]

            [dependencies]
            "foo-1.0" = [
                { output = "dev", integrity = "blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgfqq" },
                { output = "dev", integrity = "blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgfqq" },
                { output = "default", integrity = "blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgfra" }
            ]
            "bar-1.0" = [
                { output = "default", integrity = "blake3-gezdgnbvgy3tqojqgezdgnbvgy3tqojqgezdgnbvgy3tqojqgfrq" }
            ]
        "##,
        )?;
        assert_eq!(
            Spec::builder("foo")
                .add_output("default")
                .package(
                    "foo-1.0",
                    "dev",
                    nck_hashing::SupportedHash::Blake3(*b"1234567890123456789012345678901a")
                )
                .package(
                    "foo-1.0",
                    "default",
                    nck_hashing::SupportedHash::Blake3(*b"1234567890123456789012345678901b")
                )
                .package(
                    "bar-1.0",
                    "default",
                    nck_hashing::SupportedHash::Blake3(*b"1234567890123456789012345678901c")
                )
                .file(nck_hashing::SupportedHash::Blake3(
                    *b"1234567890123456789012345678901d"
                ))
                .exec("/bar", vec![])
                .build()?,
            value
        );
        Ok(())
    }
}
