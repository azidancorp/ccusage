import os
import json
import glob
import re
from datetime import datetime
from collections import defaultdict

# Paths
BRAIN_DIR = "/home/azidan/.gemini/antigravity-cli/brain"
LOG_DIR = "/home/azidan/.gemini/antigravity-cli/log"

def parse_iso_datetime(dt_str):
    if not dt_str:
        return None
    # Strip 'Z' and trailing offset info if any, parse basic formats
    dt_str = dt_str.replace("Z", "+00:00")
    try:
        return datetime.fromisoformat(dt_str)
    except ValueError:
        # Fallback for alternative formats
        try:
            return datetime.strptime(dt_str, "%Y-%m-%dT%H:%M:%S.%f")
        except ValueError:
            try:
                return datetime.strptime(dt_str, "%Y-%m-%dT%H:%M:%S")
            except ValueError:
                return None

# Step 1: Parse CLI Log Files to map PIDs and conversation IDs to models
print("Scanning CLI log files for conversation and model mapping...")
log_files = glob.glob(os.path.join(LOG_DIR, "cli-*.log"))

# Map: PID -> set of models requested in that log file
pid_to_models = defaultdict(lambda: defaultdict(int))
# Map: Conversation ID -> PID in a specific log file / run
conv_to_pids = {}
# Map: Conversation ID -> list of log files it appears in
conv_to_logs = defaultdict(list)

# Regex patterns
created_conv_pat = re.compile(r"Created conversation ([a-f0-9\-]+)")
streaming_conv_pat = re.compile(r"Streaming conversation ([a-f0-9\-]+)")
found_conv_pat = re.compile(r"found conversation ([a-f0-9\-]+)")
model_pat = re.compile(r"models/([a-zA-Z0-9\.\-_]+):streamGenerateContent")
pid_pat = re.compile(r"^[IWE]\d{4} \d{2}:\d{2}:\d{2}\.\d+ +(\d+)")

for log_path in sorted(log_files):
    filename = os.path.basename(log_path)
    with open(log_path, "r", encoding="utf-8", errors="ignore") as lf:
        for line in lf:
            pid_match = pid_pat.match(line)
            if pid_match:
                pid = pid_match.group(1)
                
                # Check for conversation ID mentions
                conv_match = created_conv_pat.search(line) or streaming_conv_pat.search(line) or found_conv_pat.search(line)
                if conv_match:
                    conv_id = conv_match.group(1)
                    conv_to_pids[conv_id] = pid
                    if filename not in conv_to_logs[conv_id]:
                        conv_to_logs[conv_id].append(filename)
                
                # Check for model requests
                model_match = model_pat.search(line)
                if model_match:
                    model_name = model_match.group(1)
                    pid_to_models[pid][model_name] += 1

# Map each Conversation ID to the most likely model requested under its PID
conv_mapped_model = {}
for conv_id, pid in conv_to_pids.items():
    models_dict = pid_to_models.get(pid)
    if models_dict:
        # Get the model with highest count for that PID
        most_common_model = max(models_dict, key=models_dict.get)
        conv_mapped_model[conv_id] = most_common_model

# Step 2: Analyze Conversations from Brain directory
print("Analyzing conversation transcripts...")
brain_dirs = glob.glob(os.path.join(BRAIN_DIR, "*"))
conversations = []

for bdir in sorted(brain_dirs):
    conv_id = os.path.basename(bdir)
    # Validate UUID format
    if not re.match(r"^[a-f0-9\-]{36}$", conv_id):
        continue
    
    transcript_path = os.path.join(bdir, ".system_generated/logs/transcript_full.jsonl")
    if not os.path.exists(transcript_path):
        transcript_path = os.path.join(bdir, ".system_generated/logs/transcript.jsonl")
    
    if not os.path.exists(transcript_path):
        print(f"Transcript not found for conversation {conv_id}")
        continue
        
    # Analyze steps
    steps = []
    with open(transcript_path, "r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            try:
                steps.append(json.loads(line))
            except Exception as e:
                pass
                
    if not steps:
        print(f"Empty transcript for conversation {conv_id}")
        continue
        
    start_time_str = steps[0].get("created_at")
    end_time_str = steps[-1].get("created_at")
    
    start_dt = parse_iso_datetime(start_time_str)
    end_dt = parse_iso_datetime(end_time_str)
    
    if start_dt and end_dt:
        duration_sec = (end_dt - start_dt).total_seconds()
    else:
        duration_sec = 0.0
        
    # Step categories and character counts
    step_types = defaultdict(int)
    source_counts = defaultdict(int)
    
    user_prompts_chars = 0
    model_responses_chars = 0
    tool_execution_steps = 0
    tool_calls_count = 0
    
    # Check for setting changes for model
    setting_models = []
    
    for step in steps:
        stype = step.get("type", "UNKNOWN")
        source = step.get("source", "UNKNOWN")
        step_types[stype] += 1
        source_counts[source] += 1
        
        content = step.get("content", "") or ""
        thinking = step.get("thinking", "") or ""
        tool_calls = step.get("tool_calls", []) or []
        
        # Check for Model Selection change
        if "<USER_SETTINGS_CHANGE>" in content:
            m = re.search(r"Model Selection` from [^ ]+ to ([^.]+)", content)
            if m:
                setting_models.append(m.group(1).strip())
        
        # Classify and accumulate
        if stype == "USER_INPUT":
            user_prompts_chars += len(content)
        elif stype == "PLANNER_RESPONSE":
            model_responses_chars += len(thinking) + len(content)
            tool_calls_count += len(tool_calls)
        elif source == "MODEL" and stype != "PLANNER_RESPONSE":
            tool_execution_steps += 1
            # Some tool output length could be included or excluded depending on criteria. 
            # The prompt asks for "input (user prompts) and output (model responses/thoughts)".
            # So tool outputs are excluded from user prompts and model responses, which is correct.
            
    # Resolve Model Used
    model_used = "gemini-3.5-flash"  # Default
    # Check if mapped from logs
    if conv_id in conv_mapped_model:
        model_used = conv_mapped_model[conv_id]
    elif setting_models:
        model_used = setting_models[-1]
        
    # Standardize model names for display / pricing
    model_used_clean = model_used.lower()
    if "lite" in model_used_clean or "3.1-flash-lite" in model_used_clean:
        model_name_display = "gemini-3.1-flash-lite"
    elif "pro" in model_used_clean:
        model_name_display = "gemini-3.5-pro"
    else:
        model_name_display = "gemini-3.5-flash"
        
    # Tokens Heuristic
    # Heuristic: 3.8 characters per token. 
    # Add 2000 tokens for the initial system prompt of each conversation.
    # Add 500 tokens overhead per tool execution step.
    input_prompt_tokens = user_prompts_chars / 3.8
    system_prompt_tokens = 2000.0
    tool_overhead_tokens = tool_execution_steps * 500.0
    
    total_input_tokens = input_prompt_tokens + system_prompt_tokens + tool_overhead_tokens
    total_output_tokens = model_responses_chars / 3.8
    
    # Cost calculation: $1.50/M tokens for input, $9.00/M tokens for output, with 90% discount ($0.15/M) for cached system prompt
    input_prompt_cost = (input_prompt_tokens / 1000000.0) * 1.50
    system_prompt_cost = (system_prompt_tokens / 1000000.0) * 0.15  # 90% caching discount
    tool_overhead_cost = (tool_overhead_tokens / 1000000.0) * 1.50
    input_cost = input_prompt_cost + system_prompt_cost + tool_overhead_cost
    output_cost = (total_output_tokens / 1000000.0) * 9.00
    total_cost = input_cost + output_cost
    
    conversations.append({
        "id": conv_id,
        "start_time": start_time_str,
        "end_time": end_time_str,
        "duration_sec": duration_sec,
        "total_steps": len(steps),
        "step_types": dict(step_types),
        "source_counts": dict(source_counts),
        "user_prompts_chars": user_prompts_chars,
        "model_responses_chars": model_responses_chars,
        "tool_execution_steps": tool_execution_steps,
        "tool_calls_count": tool_calls_count,
        "model_used": model_name_display,
        "raw_model_detected": model_used,
        "input_tokens": total_input_tokens,
        "output_tokens": total_output_tokens,
        "input_cost": input_cost,
        "output_cost": output_cost,
        "total_cost": total_cost,
        "logs": conv_to_logs.get(conv_id, [])
    })

# Output raw results as JSON
output_data = {
    "num_conversations": len(conversations),
    "conversations": conversations
}

# Write raw results JSON to scratch directory
output_json_path = "/home/azidan/.gemini/antigravity-cli/brain/d725ae7d-c9d0-402f-9458-ce5f5ea0a377/scratch/analysis_data.json"
with open(output_json_path, "w", encoding="utf-8") as jf:
    json.dump(output_data, jf, indent=2)

print(f"\nSuccessfully wrote raw analysis data to {output_json_path}")

# Format nice output
print("\n" + "="*80)
print(f"ANALYSIS SUMMARY OF {len(conversations)} CONVERSATIONS")
print("="*80)

agg_input_chars = 0
agg_output_chars = 0
agg_input_tokens = 0
agg_output_tokens = 0
agg_steps = 0
agg_tool_calls = 0
agg_tool_steps = 0
agg_input_cost = 0.0
agg_output_cost = 0.0
agg_cost = 0.0

for idx, c in enumerate(conversations):
    duration_str = f"{c['duration_sec']/60:.2f} mins" if c['duration_sec'] >= 60 else f"{c['duration_sec']:.1f} secs"
    print(f"\n{idx+1}. Conversation ID: {c['id']}")
    print(f"   Start Time : {c['start_time']}")
    print(f"   Duration   : {duration_str} ({c['duration_sec']:.1f} s)")
    print(f"   Model Used : {c['model_used']} (Detected: {c['raw_model_detected']})")
    print(f"   Steps Count: {c['total_steps']}")
    print(f"   Step Types : {c['step_types']}")
    print(f"   Tool Runs  : {c['tool_execution_steps']} steps, with {c['tool_calls_count']} total tool calls")
    print(f"   Characters : Input (Prompts): {c['user_prompts_chars']:,} | Output (Model): {c['model_responses_chars']:,}")
    print(f"   Est. Tokens: Input: {c['input_tokens']:.1f} | Output: {c['output_tokens']:.1f} (Total: {c['input_tokens'] + c['output_tokens']:.1f})")
    print(f"   Est. Cost  : Input: ${c['input_cost']:.6f} | Output: ${c['output_cost']:.6f} | Total: ${c['total_cost']:.6f}")
    
    agg_input_chars += c['user_prompts_chars']
    agg_output_chars += c['model_responses_chars']
    agg_input_tokens += c['input_tokens']
    agg_output_tokens += c['output_tokens']
    agg_steps += c['total_steps']
    agg_tool_calls += c['tool_calls_count']
    agg_tool_steps += c['tool_execution_steps']
    agg_input_cost += c['input_cost']
    agg_output_cost += c['output_cost']
    agg_cost += c['total_cost']

print("\n" + "="*80)
print("AGGREGATED ESTIMATES (GRAND TOTALS)")
print("="*80)
print(f"Total Conversations     : {len(conversations)}")
print(f"Total Steps (All types) : {agg_steps}")
print(f"Total Tool Executions   : {agg_tool_steps}")
print(f"Total Tool Calls        : {agg_tool_calls}")
print(f"Total Input Characters  : {agg_input_chars:,}")
print(f"Total Output Characters : {agg_output_chars:,}")
print(f"Total Estimated Tokens  : Input: {agg_input_tokens:,.1f} | Output: {agg_output_tokens:,.1f} (Total: {agg_input_tokens+agg_output_tokens:,.1f})")
print(f"Total Estimated Cost    : Input: ${agg_input_cost:.4f} | Output: ${agg_output_cost:.4f} | Total: ${agg_cost:.4f}")
print("="*80)
