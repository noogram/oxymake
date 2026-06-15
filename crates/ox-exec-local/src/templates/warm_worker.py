#!/usr/bin/env python3
"""OxyMake warm worker — fork-after-import dispatch loop.

Architecture:
  1. Import phase: load all codec libraries (numpy, pandas, etc.)
  2. Signal "ready" to parent via stdout
  3. Dispatch loop: for each JSON command on stdin, fork() a child
     that inherits imported modules via COW, executes, and exits.

The fork-after-import pattern gives:
  - Warm imports: 0ms per job after the first (modules in COW pages)
  - State isolation by construction: each dispatch in a fresh process
  - No GIL issues: template is single-threaded at fork time

Protocol (JSON-line on stdin/stdout):
  → {"status": "ready"}                          (worker → parent)
  ← {"cmd": "exec", "module": "...", ...}        (parent → worker)
  → {"status": "ok"} | {"status": "error", ...}  (worker → parent)
  ← {"cmd": "shutdown"}                          (parent → worker)
"""
import importlib
import json
import os
import signal
import sys
import traceback

# === GENERATED IMPORTS (injected by Rust) ===
# {WARM_IMPORTS}

# === CODEC READERS/WRITERS (injected by Rust) ===
_CODEC_READERS = {
    # {CODEC_READERS}
}
_CODEC_WRITERS = {
    # {CODEC_WRITERS}
}

# Signal readiness to parent.
sys.stdout.write('{"status":"ready"}\n')
sys.stdout.flush()

# === DISPATCH LOOP ===
for raw_line in sys.stdin:
    raw_line = raw_line.strip()
    if not raw_line:
        continue

    try:
        msg = json.loads(raw_line)
    except json.JSONDecodeError as e:
        sys.stdout.write(json.dumps({"status": "error", "msg": f"json decode: {e}"}) + "\n")
        sys.stdout.flush()
        continue

    cmd = msg.get("cmd")

    if cmd == "shutdown":
        break

    if cmd != "exec":
        sys.stdout.write(json.dumps({"status": "error", "msg": f"unknown cmd: {cmd}"}) + "\n")
        sys.stdout.flush()
        continue

    # Fork a child for this dispatch — inherits imported modules via COW.
    r_fd, w_fd = os.pipe()
    pid = os.fork()

    if pid == 0:
        # === CHILD PROCESS ===
        os.close(r_fd)
        # Redirect stdout to stderr so user print() doesn't corrupt protocol.
        _real_stdout = sys.stdout
        sys.stdout = sys.stderr
        try:
            module_path = msg["module"]
            func_name = msg["function"]
            input_specs = msg.get("inputs", [])
            output_specs = msg.get("outputs", [])
            params = msg.get("params", {})

            # Import target module.
            mod = importlib.import_module(module_path)
            func = getattr(mod, func_name)

            # Deserialize inputs.
            kwargs = {}
            for inp in input_specs:
                codec = inp["codec"]
                reader = _CODEC_READERS.get(codec)
                if reader:
                    kwargs[inp["var"]] = reader(inp["path"])
                else:
                    # Fallback: read raw bytes.
                    with open(inp["path"], "rb") as f:
                        kwargs[inp["var"]] = f.read()
            kwargs.update(params)

            # Call the function.
            result = func(**kwargs)

            # Serialize outputs.
            if len(output_specs) == 1:
                out = output_specs[0]
                writer = _CODEC_WRITERS.get(out["codec"])
                if writer:
                    writer(result, out["path"])
                else:
                    with open(out["path"], "wb") as f:
                        f.write(result if isinstance(result, bytes) else str(result).encode())
            else:
                for out in output_specs:
                    name = out.get("name")
                    val = result[name] if isinstance(result, dict) and name else result
                    writer = _CODEC_WRITERS.get(out["codec"])
                    if writer:
                        writer(val, out["path"])

            os.write(w_fd, b'{"status":"ok"}\n')
        except Exception as e:
            err = json.dumps({
                "status": "error",
                "msg": f"{type(e).__name__}: {e}",
                "traceback": traceback.format_exc(),
            })
            os.write(w_fd, (err + "\n").encode())
        finally:
            os.close(w_fd)
            os._exit(0)  # _exit, not sys.exit — skip atexit handlers
    else:
        # === PARENT PROCESS ===
        os.close(w_fd)
        # Read child result with timeout.
        try:
            data = os.read(r_fd, 4_000_000)  # 4MB max response
            os.close(r_fd)
            _, status = os.waitpid(pid, 0)

            if data:
                sys.stdout.write(data.decode())
            elif os.WIFSIGNALED(status):
                sig = os.WTERMSIG(status)
                sys.stdout.write(json.dumps({
                    "status": "error",
                    "msg": f"child killed by signal {sig}",
                }) + "\n")
            else:
                exit_code = os.WEXITSTATUS(status)
                sys.stdout.write(json.dumps({
                    "status": "error",
                    "msg": f"child exited with code {exit_code} but no output",
                }) + "\n")
        except Exception as e:
            sys.stdout.write(json.dumps({
                "status": "error",
                "msg": f"parent error: {e}",
            }) + "\n")
            # Kill child if still alive.
            try:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            except ProcessLookupError:
                pass
        sys.stdout.flush()
