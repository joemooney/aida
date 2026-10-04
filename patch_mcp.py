import re

with open('aida-cli-lib/src/mcp.rs', 'r') as f:
    content = f.read()

read_tool_handlers = [
    'list_requirements',
    'show_requirement',
    'file_finding',
    'read_brief',
    'list_briefs',
    'get_protocol',
    'list_protocols',
]

out = []
in_read_handler = False
for line in content.split('\n'):
    match = re.search(r'async fn ([a-z_]+)\(', line)
    if match:
        func = match.group(1)
        in_read_handler = func in read_tool_handlers
    
    if in_read_handler:
        line = line.replace('storage.load()', 'storage.load_for_read()')
        line = line.replace('backend.list_requirements', 'backend.list_requirements_for_read')
        line = line.replace('self.storage.list_requirements', 'self.storage.list_requirements_for_read')
    out.append(line)

with open('aida-cli-lib/src/mcp.rs', 'w') as f:
    f.write('\n'.join(out))
