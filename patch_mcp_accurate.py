import re

read_tools = [
    'tool_list_requirements',
    'tool_show_requirement',
    'tool_search_requirements',
    'tool_query_graph',
    'tool_history',
    'tool_read_inbox',
    'tool_list_features',
    'tool_list_punts',
    'tool_read_punt',
    'tool_list_findings',
    'tool_list_active_leases',
    'tool_list_directives',
    'tool_list_briefs',
    'tool_read_brief',
    'tool_queue_list',
    'tool_session_leases',
    'tool_session_status',
    'tool_session_manifest',
    'tool_role_list',
    'tool_role_show',
    'tool_cache_status',
    'tool_status_unified',
    'tool_usage_query',
    'tool_schema'
]

with open('aida-cli-lib/src/mcp.rs', 'r') as f:
    content = f.read()

out = []
in_read_tool = False
for line in content.split('\n'):
    if 'fn tool_' in line:
        # check if it's in our read_tools list
        tool_match = re.search(r'fn (tool_[a-z_]+)\(', line)
        if tool_match:
            in_read_tool = tool_match.group(1) in read_tools
        else:
            in_read_tool = False
    
    if in_read_tool:
        line = line.replace('storage.load()', 'storage.load_for_read()')
    
    out.append(line)

with open('aida-cli-lib/src/mcp.rs', 'w') as f:
    f.write('\n'.join(out))

