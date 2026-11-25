-- Beryl's own development environment (`beryl --daemon develop`)
local p = beryl.nixpkgs

return {
  description = "Beryl development environment",

  shells = {
    default = beryl.shell({
      packages = {
        -- Rust toolchain
        p("rustc"),
        p("cargo"),
        p("rust-analyzer"),
        p("clippy"),
        p("rustfmt"),

        -- C / C++ toolchain & build tools
        p("gcc"),
        p("clang"),
        p("cmake"),
        p("gnumake"),
        p("pkg-config"),
        p("gdb"),
      },

      libs = {
        p("lua5_4"),
        p("sqlite"),
        p("sqlite.dev"),
        p("openssl"),
        p("openssl.dev"),
        p("xz"),
        p("xz.dev"),
        p("zstd"),
        p("zstd.dev"),
      },

      env = {
        LIBCLANG_PATH = p("libclang.lib") .. "/lib",
        RUST_BACKTRACE = "1",
      },

      hook = [[
        echo "Beryl dev shell ready ($(rustc --version))"
      ]],
    }),

    docs = beryl.shell({
      packages = {
        p("mdbook"),
      },
      hook = [[
        echo "Beryl docs shell ready ($(mdbook --version))"
      ]],
    }),
  },
}
