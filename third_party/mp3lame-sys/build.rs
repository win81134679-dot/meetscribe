//! MeetScribe 修改版：上游在 Unix 目標走 autotools、在 Windows 主機走 cc，
//! 但 build-dependencies 依「目標」挑選，從 Windows 交叉編譯到 Android 時兩邊對不上（拿不到 cc）。
//! 這裡一律用 cc 直接編 libmp3lame，MSVC 用上游的 configMS.h，其他（Android/Linux）用下面產生的 config.h。

const LAME_DIR: &str = "lame-3.100";

const FILES: &[&str] = &[
    "bitstream.c",
    "encoder.c",
    "fft.c",
    "gain_analysis.c",
    "id3tag.c",
    "lame.c",
    "newmdct.c",
    "presets.c",
    "psymodel.c",
    "quantize_pvt.c",
    "quantize.c",
    "reservoir.c",
    "set_get.c",
    "tables.c",
    "takehiro.c",
    "util.c",
    "vbrquantize.c",
    "VbrTag.c",
    "version.c",
];

fn main() {
    if std::env::var("DOCS_RS").is_ok_and(|d| d == "1") {
        return;
    }
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let pointer_width = std::env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap_or_else(|_| "64".into());
    let lame_dir = std::path::Path::new(LAME_DIR);
    let include = out_dir.join("include_config");
    std::fs::create_dir_all(&include).expect("create include dir");

    if target_env == "msvc" {
        std::fs::copy(lame_dir.join("configMS.h"), include.join("config.h")).expect("copy config.h");
    } else {
        let long = if pointer_width == "64" { 8 } else { 4 };
        let config = format!(
            "#ifndef LAME_CONFIG_H\n#define LAME_CONFIG_H\n\
             #define STDC_HEADERS 1\n#define HAVE_ERRNO_H 1\n#define HAVE_FCNTL_H 1\n#define HAVE_LIMITS_H 1\n\
             #define HAVE_STDINT_H 1\n#define HAVE_INTTYPES_H 1\n#define HAVE_STRING_H 1\n#define HAVE_STDLIB_H 1\n\
             #define HAVE_UNISTD_H 1\n#define HAVE_STRCHR 1\n#define HAVE_MEMCPY 1\n\
             #define PACKAGE \"lame\"\n#define PROTOTYPES 1\n#define USE_FAST_LOG 1\n#define LAME_LIBRARY_BUILD 1\n\
             #define SIZEOF_SHORT 2\n#define SIZEOF_UNSIGNED_SHORT 2\n#define SIZEOF_INT 4\n#define SIZEOF_UNSIGNED_INT 4\n\
             #define SIZEOF_LONG {long}\n#define SIZEOF_UNSIGNED_LONG {long}\n#define SIZEOF_FLOAT 4\n#define SIZEOF_DOUBLE 8\n\
             #include <stdint.h>\ntypedef float ieee754_float32_t;\n#endif\n"
        );
        std::fs::write(include.join("config.h"), config).expect("write config.h");
    }

    let mut cc = cc::Build::new();
    for f in FILES {
        cc.file(lame_dir.join("libmp3lame").join(f));
    }
    // SSE 版量化只在 x86 有意義
    if target_arch == "x86_64" || target_arch == "x86" {
        cc.file(lame_dir.join("libmp3lame/vector/xmm_quantize_sub.c"));
    }
    cc.include(lame_dir.join("include"))
        .include(&include)
        .include(lame_dir.join("libmp3lame"))
        .define("TAKEHIRO_IEEE754_HACK", None)
        .define("FLOAT8", Some("float"))
        .define("REAL_IS_FLOAT", Some("1"))
        .define("BS_FORMAT", Some("BINARY"))
        .define("HAVE_CONFIG_H", None)
        .warnings(false)
        .extra_warnings(false);
    if target_env == "msvc" {
        cc.pic(false);
    } else {
        cc.pic(true).flag_if_supported("-w");
    }
    cc.compile("mp3lame");
    println!("cargo:rerun-if-changed=build.rs");
}
