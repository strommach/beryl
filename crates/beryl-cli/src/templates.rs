//! Project scaffold templates for `beryl init [template]`.

pub struct Template {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub content: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "minimal",
        aliases: &["default", "basic"],
        summary: "Minimal starter beryl.lua dev shell",
        content: r#"-- Beryl project manifest (`beryl develop` to enter the dev shell)
local p = beryl.nixpkgs

return {
  description = "Minimal Beryl project",

  shells = {
    default = beryl.shell({
      packages = {
        p("git"),
        p("ripgrep"),
      },
      env = {
        PROJECT_ENV = "dev",
      },
      hook = [[
        echo "Entered Beryl dev shell"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "rust",
        aliases: &["rs"],
        summary: "Rust toolchain (rustc, cargo, rust-analyzer, clippy, rustfmt)",
        content: r#"-- Rust development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "Rust project",

  shells = {
    default = beryl.shell({
      packages = {
        p("rustc"),
        p("cargo"),
        p("rust-analyzer"),
        p("clippy"),
        p("rustfmt"),
        p("pkg-config"),
        p("gcc"),
      },
      libs = {
        p("openssl"),
      },
      env = {
        RUST_BACKTRACE = "1",
      },
      hook = [[
        echo "Rust dev shell ready ($(rustc --version))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "c",
        aliases: &[],
        summary: "C toolchain (gcc, clang, make, cmake, pkg-config, gdb)",
        content: r#"-- C development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "C project",

  shells = {
    default = beryl.shell({
      packages = {
        p("gcc"),
        p("clang"),
        p("gnumake"),
        p("cmake"),
        p("pkg-config"),
        p("gdb"),
        p("clang-tools"),
      },
      libs = {
        -- Add C libraries here, e.g. p("zlib"), p("openssl"), p("sqlite")
      },
      env = {
        CC = "gcc",
      },
      hook = [[
        echo "C dev shell ready ($(gcc --version | head -n 1))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "cpp",
        aliases: &["c++", "cxx"],
        summary: "C++ toolchain (gcc, clang, cmake, ninja, gdb, clang-tools)",
        content: r#"-- C++ development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "C++ project",

  shells = {
    default = beryl.shell({
      packages = {
        p("gcc"),
        p("clang"),
        p("cmake"),
        p("ninja"),
        p("gnumake"),
        p("pkg-config"),
        p("gdb"),
        p("clang-tools"),
      },
      libs = {
        -- Add C/C++ libraries here, e.g. p("zlib"), p("openssl"), p("fmt")
      },
      env = {
        CXX = "g++",
        CMAKE_GENERATOR = "Ninja",
      },
      hook = [[
        echo "C++ dev shell ready ($(g++ --version | head -n 1))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "python",
        aliases: &["py"],
        summary: "Python environment (python3, uv, ruff, pyright)",
        content: r#"-- Python development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "Python project",

  shells = {
    default = beryl.shell({
      packages = {
        p("python3"),
        p("uv"),
        p("ruff"),
        p("pyright"),
      },
      env = {
        PYTHONDONTWRITEBYTECODE = "1",
      },
      hook = [[
        echo "Python dev shell ready ($(python3 --version))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "node",
        aliases: &["js", "ts", "typescript"],
        summary: "Node.js & TypeScript (nodejs, pnpm, typescript, tsserver)",
        content: r#"-- Node.js / TypeScript development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "Node.js project",

  shells = {
    default = beryl.shell({
      packages = {
        p("nodejs"),
        p("pnpm"),
        p("typescript"),
        p("typescript-language-server"),
      },
      env = {
        NODE_ENV = "development",
      },
      hook = [[
        echo "Node dev shell ready (node $(node --version), pnpm $(pnpm --version))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "web",
        aliases: &["frontend"],
        summary: "Web frontend stack (nodejs, pnpm, typescript, tailwindcss, esbuild)",
        content: r#"-- Web development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "Web project",

  shells = {
    default = beryl.shell({
      packages = {
        p("nodejs"),
        p("pnpm"),
        p("typescript"),
        p("typescript-language-server"),
        p("tailwindcss"),
        p("esbuild"),
      },
      env = {
        NODE_ENV = "development",
      },
      hook = [[
        echo "Web dev shell ready (node $(node --version))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "java",
        aliases: &["jvm"],
        summary: "Java / JVM toolchain (jdk, maven, gradle, jdt-language-server)",
        content: r#"-- Java development environment (`beryl develop`)
local p = beryl.nixpkgs
local jdk = p("jdk")

return {
  description = "Java project",

  shells = {
    default = beryl.shell({
      packages = {
        jdk,
        p("maven"),
        p("gradle"),
        p("jdt-language-server"),
      },
      env = {
        JAVA_HOME = jdk,
      },
      hook = [[
        echo "Java dev shell ready ($(java -version 2>&1 | head -n 1))"
      ]],
    }),
  },
}
"#,
    },
    Template {
        name: "go",
        aliases: &["golang"],
        summary: "Go toolchain (go, gopls, golangci-lint)",
        content: r#"-- Go development environment (`beryl develop`)
local p = beryl.nixpkgs

return {
  description = "Go project",

  shells = {
    default = beryl.shell({
      packages = {
        p("go"),
        p("gopls"),
        p("golangci-lint"),
      },
      env = {
        CGO_ENABLED = "1",
      },
      hook = [[
        echo "Go dev shell ready ($(go version))"
      ]],
    }),
  },
}
"#,
    },
];

pub fn find_template(query: &str) -> Option<&'static Template> {
    let q = query.to_ascii_lowercase();
    TEMPLATES
        .iter()
        .find(|t| t.name == q || t.aliases.contains(&q.as_str()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use beryl_eval::{Evaluator, NixpkgsLookup};
    use beryl_nix::store_path::StorePath;
    use beryl_nix::store_path_ext::parse_store_path;

    use super::*;

    struct DummyLookup;

    impl NixpkgsLookup for DummyLookup {
        fn resolve_nixpkgs(&self, _system: &str, attr: &str) -> beryl_eval::Result<StorePath> {
            let safe: String = attr
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let sp =
                parse_store_path(&format!("/nix/store/00bgd045z0d4icpbc2yyz4gx48ak44la-{safe}"))?;
            Ok(sp)
        }
    }

    #[test]
    fn all_templates_evaluate_cleanly() {
        let evaluator = Evaluator::new("aarch64-linux", Arc::new(DummyLookup));
        for tpl in TEMPLATES {
            let manifest = evaluator
                .eval_project_str(tpl.content, tpl.name)
                .unwrap_or_else(|e| panic!("template '{}' failed to evaluate: {e}", tpl.name));
            assert!(
                manifest.shells.contains_key("default"),
                "template '{}' must define a `default` shell",
                tpl.name
            );
        }
    }
}
