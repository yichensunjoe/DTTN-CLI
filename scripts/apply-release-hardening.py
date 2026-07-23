from pathlib import Path
import textwrap


def main() -> None:
    workflow = Path(".github/workflows/one-shot-release-hardening.yml")
    source = workflow.read_text()
    marker = "          python3 <<'PY'\n"
    start = source.index(marker) + len(marker)
    end = source.index("\n          PY\n", start)
    code = textwrap.dedent(source[start:end])

    brittle_start = code.index("update_arm = re.compile")
    brittle_end = code.index("main_path.write_text(main)", brittle_start)
    robust_patch = textwrap.dedent(
        r"""
        update_start = main.index("            Command::Update {")
        update_end_marker = "            Command::Login {"
        update_end = main.index(update_end_marker, update_start)
        replacement_arm = '''            Command::Update { json, .. } => {
                init_tracing_simple("cli");
                let _otel_guard = xai_grok_telemetry::otel_layer::otel_guard();
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "enabled": false,
                            "channel": "alpha",
                            "repository": "yichensunjoe/DTTN-CLI",
                            "message": "DTTN self-update is not enabled in this alpha release. Install the latest version from the DTTN GitHub Release."
                        })
                    );
                } else {
                    println!("DTTN self-update is not enabled in this alpha release.");
                    println!("Install the latest version from the DTTN GitHub Release:");
                    println!("  https://github.com/yichensunjoe/DTTN-CLI/releases");
                }
                return Ok(());
            }
'''
        main = main[:update_start] + replacement_arm + main[update_end:]
        """
    )
    code = code[:brittle_start] + robust_patch + code[brittle_end:]
    exec(compile(code, "release-hardening-embedded.py", "exec"), {})

    Path(".github/workflows/one-shot-release-hardening-pr.yml").unlink()
    Path("scripts/apply-release-hardening.py").unlink()


if __name__ == "__main__":
    main()
