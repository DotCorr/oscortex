import json

transcript_path = "/Users/ghostportal/.gemini/antigravity/brain/5e6dff7c-2846-4ab3-9cf0-9d540ac9861a/.system_generated/logs/transcript.jsonl"

with open(transcript_path, "r") as f:
    for line in f:
        try:
            data = json.loads(line)
            step = data.get("step_index", -1)
            if 7900 <= step <= 8050:
                source = data.get("source", "")
                if source == "MODEL":
                    thinking = data.get("thinking", "")
                    print(f"--- STEP {step} ---")
                    if thinking:
                        print(thinking)
        except Exception as e:
            pass
