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
                if "Case1" in content or "Case2" in content:
                    for part in content.split("\n"):
                        if "Case1" in part or "Case2" in part:
                            print(part)
            except Exception as e:
                pass

if __name__ == '__main__':
    main()
