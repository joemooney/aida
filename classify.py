import re
import os

READ_FUNCS = [
    b'handle_graph',
    b'handle_tree',
    b'handle_digest',
    b'handle_tracker',
    b'handle_fleet',
    b'handle_burndown',
    b'handle_report',
    b'handle_metrics',
    b'handle_status',
    b'handle_history',
    b'handle_list',
    b'handle_show',
    b'handle_search',
    b'list_requirements',
    b'show_requirement',
]

READ_MCP = [
    b'list_requirements',
    b'show_requirement',
    b'file_finding',
    b'read_brief',
    b'list_briefs',
    b'get_protocol',
    b'list_protocols',
]

def process_file(path):
    with open(path, 'rb') as f:
        content = f.read()

    # Find function boundaries (roughly)
    # This is a naive regex for rust function
    func_pattern = re.compile(rb'fn\s+([a-zA-Z0-9_]+)\s*\(')
    
    out = bytearray()
    last_idx = 0
    
    current_func = None
    
    for match in re.finditer(func_pattern, content):
        func_name = match.group(1)
        # process the body of the PREVIOUS function
        start = last_idx
        end = match.start()
        
        body = content[start:end]
        if current_func:
            is_read = any(rf in current_func for rf in READ_FUNCS) or (b'mcp.rs' in path.encode() and any(rm in current_func for rm in READ_MCP))
            if is_read:
                body = body.replace(b'.load()', b'.load_for_read()')
                body = body.replace(b'.list_requirements(', b'.list_requirements_for_read(')
        
        out.extend(body)
        last_idx = end
        current_func = func_name
        
    # last func
    body = content[last_idx:]
    if current_func:
        is_read = any(rf in current_func for rf in READ_FUNCS) or (b'mcp.rs' in path.encode() and any(rm in current_func for rm in READ_MCP))
        if is_read:
            body = body.replace(b'.load()', b'.load_for_read()')
            body = body.replace(b'.list_requirements(', b'.list_requirements_for_read(')
    out.extend(body)
    
    with open(path, 'wb') as f:
        f.write(out)

for root, _, files in os.walk('aida-cli-lib/src'):
    for f in files:
        if f.endswith('.rs'):
            process_file(os.path.join(root, f))
