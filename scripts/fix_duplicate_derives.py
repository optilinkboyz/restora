import re

files = [
    "crates/restora-application/src/recovery_job.rs",
    "crates/restora-application/src/session_store.rs",
    "crates/restora-application/src/wipe_job.rs",
]

pattern = re.compile(
    r"#\[derive\(Debug, Clone\)\]\n(#\[derive\(Debug, Clone, serde::Serialize\)\]\n)"
)

for path in files:
    text = open(path).read()
    new_text, count = pattern.subn(r"\1", text)
    if count:
        open(path, "w").write(new_text)
        print(f"{path}: removed {count} duplicate derive line(s)")
    else:
        print(f"{path}: no duplicates found (already clean)")