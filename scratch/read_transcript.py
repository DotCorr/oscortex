import json
from pathlib import Path

def main():
    transcript_path = Path("/Users/ghostportal/.gemini/antigravity/brain/479b9a26-1fa6-4678-9573-e943f2cd0cbf/.system_generated/logs/transcript.jsonl")
    if not transcript_path.exists():
        print("Transcript not found")
        return

    target_steps = [3848, 3849, 3850, 3958]
    with open(transcript_path, "r") as f:
        for line in f:
            try:
                step = json.loads(line)
            except Exception:
                continue
            idx = step.get("step_index")
            if idx in target_steps or (3840 <= idx <= 3855):
                print(f"================== Step {idx} ({step.get('source')}) ==================")
                content = step.get("content", "")
                thinking = step.get("thinking", "")
                tool_calls = step.get("tool_calls", [])
                if thinking:
                    print("THINKING:\n", thinking)
                if content:
                    print("CONTENT:\n", content)
                if tool_calls:
                    print("TOOL CALLS:\n", json.dumps(tool_calls, indent=2))

if __name__ == "__main__":
    main()
