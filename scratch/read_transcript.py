import json
from pathlib import Path

def main():
    transcript_path = Path("/Users/ghostportal/.gemini/antigravity/brain/ffb0775d-2b80-4377-9009-9728c6ccecc0/.system_generated/logs/transcript.jsonl")
    if not transcript_path.exists():
        print("Transcript does not exist")
        return
        
    with open(transcript_path, 'r') as f:
        for line in f:
            try:
                obj = json.loads(line)
                step = obj.get("step_index", 0)
                if 5000 <= step <= 5200:
                    source = obj.get("source")
                    type_ = obj.get("type")
                    content = obj.get("content", "")
                    thinking = obj.get("thinking", "")
                    tool_calls = obj.get("tool_calls", [])
                    print(f"--- STEP {step} ({source}/{type_}) ---")
                    if thinking:
                        print(f"THINKING:\n{thinking}\n")
                    if content:
                        print(f"CONTENT:\n{content}\n")
                    if tool_calls:
                        print(f"TOOL CALLS: {tool_calls}\n")
            except Exception as e:
                print(f"Error parsing line: {e}")

if __name__ == '__main__':
    main()
