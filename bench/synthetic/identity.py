"""Identity function for synthetic benchmark.

Reads input, returns it unchanged. Measures the pure transport overhead
(subprocess spawn + Python import + file I/O) without any compute.
"""
import sys
import time

def main():
    t0 = time.perf_counter()
    input_path = sys.argv[1]
    output_path = sys.argv[2]

    with open(input_path, 'rb') as f:
        data = f.read()

    with open(output_path, 'wb') as f:
        f.write(data)

    elapsed = time.perf_counter() - t0
    # Print timing to stderr so ox captures it in logs
    print(f"identity: {len(data)} bytes, {elapsed*1000:.1f}ms", file=sys.stderr)

if __name__ == "__main__":
    main()
