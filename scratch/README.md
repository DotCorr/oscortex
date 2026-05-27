# scratch/

This directory previously held one-off AI-generated debug/patch scripts. They
have been removed or consolidated into:

- `tools/flutter-engine/engine_patch.py` — engine binary patches (P1–P6, P9, P10)
  and `kernel_blob.bin` SDK hash patching (`--kernel-blob`)
- `harness/verify_engine_patches.py` — smoke test for patched engine bytes

Do not add new patch scripts here; extend `engine_patch.py` instead.
