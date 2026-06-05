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
                content = obj.get("content", "")
                if "[embedder" in content or "Patching AOT" in content or "patched" in content or "pointer" in content:
                    print(f"Step {obj.get('step_index')}: {content[:300]}")
            except Exception as e:
                pass

if __name__ == '__main__':
    main()
