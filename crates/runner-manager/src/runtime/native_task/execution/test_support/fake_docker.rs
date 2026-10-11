use std::path::{Path, PathBuf};

pub(super) fn write_fake_docker(
    dir: &Path,
    log: &Path,
    inspect: &Path,
    stopped_inspect: &Path,
    other_names: &Path,
    mode: &Path,
    marker: &Path,
    barrier: &Path,
    container_name: &str,
) -> PathBuf {
    let quote = |path: &Path| shell_quote(path.to_str().expect("utf-8 fake path"));
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> {}\nmode=$(cat {})\ncheck_barrier() {{ if [ \"$mode\" = require-stop-barrier ] && ! grep -Eq '\"stopped_generation\":[1-9]' {}; then echo 'barrier missing' >&2; exit 1; fi; }}\ncase \"$1\" in\n  volume) exit 0 ;;\n  ps) check_barrier; cat {}; if [ -f {} ]; then echo {}; fi; exit 0 ;;\n  inspect) check_barrier; if [ \"$2\" = {} ] && [ -f {} ]; then cat {}; exit 0; fi; if [ \"$2\" != {} ] && grep -Fxq \"$2\" {}; then cat {}; exit 0; fi; echo 'Error: No such container' >&2; exit 1 ;;\n  run) if [ \"$mode\" = fail-run ]; then exit 1; fi; touch {}; if [ \"$mode\" = timeout-after-create ]; then sleep 1; fi; if [ \"$mode\" = slow-run ]; then sleep 0.3; fi; exit 0 ;;\n  exec) if [ \"$mode\" = fail-task-probe ] && [[ \"$@\" == *'/workspace'* ]]; then exit 1; fi; exit 0 ;;\n  stop) check_barrier; if [ \"$mode\" = stop-stuck ]; then exit 1; fi; cp {} {}; exit 0 ;;\n  rm) check_barrier; if [ \"$mode\" = stop-stuck ]; then exit 1; fi; rm -f {}; exit 0 ;;\n  *) exit 0 ;;\nesac\n",
        quote(log),
        quote(mode),
        quote(barrier),
        quote(other_names),
        quote(marker),
        shell_quote(container_name),
        shell_quote(container_name),
        quote(marker),
        quote(inspect),
        shell_quote(container_name),
        quote(other_names),
        quote(inspect),
        quote(marker),
        quote(stopped_inspect),
        quote(inspect),
        quote(marker),
    );
    crate::runtime::execution_environment::test_support::write_executable(
        dir,
        "docker-fake",
        &script,
    )
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}
