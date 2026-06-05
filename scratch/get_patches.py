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
                tool_calls = obj.get("tool_calls", [])
                for call in tool_calls:
                    if call.get("name") == "multi_replace_file_content" and "flutter-embedder/src/main.rs" in json.dumps(call.get("args")):
                        step = obj.get("step_index")
                        out_path = Path(f"/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch/patch_{step}.json")
                        with open(out_path, 'w') as out_f:
                            json.dump(call.get("args"), out_f, indent=2)
                        print(f"Wrote {out_path}")
            except Exception as e:
                pass

if __name__ == '__main__':
    main()
