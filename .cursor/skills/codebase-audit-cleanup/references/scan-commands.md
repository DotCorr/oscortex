# Scan Commands — OSCortex

Run from repo root. Always exclude `./landing/` and `./.git/`.

## Universal

```bash
find . -type f \( -name "*.rs" -o -name "*.dart" -o -name "*.py" -o -name "*.sh" \) \
  ! -path "./.git/*" ! -path "./target/*" ! -path "./build/*" ! -path "./landing/*" \
  | xargs wc -l 2>/dev/null | sort -rn | head -40

grep -rn "TODO\|FIXME\|HACK\|XXX\|TEMP\|WORKAROUND" \
  --include="*.rs" --include="*.dart" --include="*.py" --include="*.sh" . \
  | grep -v "./.git" | grep -v "./landing"
```

## Rust (kernel / embedder)

```bash
cd kernel && cargo check 2>&1 | grep -E "warning:|unused"
cd kernel && cargo clippy -- -W dead_code 2>&1 | head -40
grep -rn "todo!\(\)\|unimplemented!\(\)" kernel/ tools/flutter-embedder/
```

## Dart / Flutter

```bash
find apps -name "*.dart" ! -path "*/landing/*"
grep -rn "print(" apps/ --include="*.dart"
```

## Python (engine tooling)

```bash
grep -rn "def va_to_file\|PATCHES\[" tools/ --include="*.py"
python3 harness/verify_engine_patches.py
```

## Quick audit script

```bash
bash -c '
echo "=== OSCortex Quick Audit ==="
echo "--- Top files ---"
find . -type f \( -name "*.rs" -o -name "*.dart" -o -name "*.py" \) \
  ! -path "./.git/*" ! -path "./target/*" ! -path "./landing/*" \
  | xargs wc -l 2>/dev/null | sort -rn | head -15
echo "--- scratch ---"
ls scratch 2>/dev/null || echo "(none)"
echo "--- debt markers ---"
grep -rn "TODO\|FIXME\|HACK" --include="*.rs" --include="*.dart" . \
  | grep -v "./.git" | grep -v "./landing" | wc -l
'
```
