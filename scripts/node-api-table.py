import json
import sys

source, target = sys.argv[1], sys.argv[2]
table = json.load(open(source, encoding='utf-8'))
rows = [f"docs\t{table['node_docs_version']}\t\t"]
for kind in ('modules', 'members', 'globals'):
    for name, entry in sorted(table[kind].items()):
        versions = sorted(set(entry.get('added_all', [entry['added']])), key=lambda v: [int(x) for x in v.split('.')])
        unflagged = entry.get('unflagged', '')
        prefixed = 'node:' if kind == 'modules' and entry.get('node_prefix_only') else ''
        rows.append(f"{kind[:-1]}\t{prefixed}{name}\t{','.join(versions)}\t{unflagged}")
open(target, 'w', encoding='utf-8', newline='\n').write('\n'.join(rows) + '\n')
print(len(rows) - 1, 'entries')
