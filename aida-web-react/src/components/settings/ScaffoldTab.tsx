import { useState } from 'react';

function generateCompose(projectName: string, port: string): string {
  const safeName = projectName.toLowerCase().replace(/[^a-z0-9-]/g, '-') || 'my-project';
  return `# AIDA for ${projectName || 'my-project'}
# Run: docker compose -f .aida/docker-compose.yml up
services:
  aida:
    image: ghcr.io/joemooney/aida:latest
    container_name: aida-${safeName}
    ports:
      - "${port || '8080'}:8080"
    volumes:
      - ..:/repo
    command:
      - /app/aida-server
      - --host=0.0.0.0
      - --rest-port=8080
      - --database=/repo/requirements.db
      - --static-dir=/app/static
    environment:
      RUST_LOG: info
      # ANTHROPIC_API_KEY: sk-ant-...  # Uncomment for AI chat
`;
}

export function ScaffoldTab() {
  const [projectName, setProjectName] = useState('');
  const [port, setPort] = useState('8080');
  const [copied, setCopied] = useState(false);

  const yaml = generateCompose(projectName, port);
  const displayPort = port || '8080';

  const handleCopy = async () => {
    await navigator.clipboard.writeText(yaml);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="max-w-lg flex flex-col gap-5">
      <p className="text-sm text-content-secondary">
        Generate a <code className="text-xs bg-surface-raised px-1 py-0.5 rounded">docker-compose.yml</code> to add AIDA to an existing project.
      </p>

      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-content-secondary">Project Name</span>
        <input
          type="text"
          value={projectName}
          onChange={(e) => setProjectName(e.target.value)}
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
          placeholder="e.g., my-project"
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-xs font-medium text-content-secondary">Host Port</span>
        <input
          type="text"
          value={port}
          onChange={(e) => setPort(e.target.value)}
          className="rounded-lg border border-edge bg-surface px-3 py-2 text-sm text-content placeholder-content-muted focus:border-accent focus:outline-none"
          placeholder="8080"
        />
      </label>

      {/* Generated YAML */}
      <div className="flex flex-col gap-1">
        <div className="flex items-center justify-between">
          <span className="text-xs font-medium text-content-secondary">.aida/docker-compose.yml</span>
          <button
            onClick={handleCopy}
            className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors"
          >
            {copied ? 'Copied!' : 'Copy'}
          </button>
        </div>
        <pre className="rounded-lg border border-edge bg-surface-raised px-4 py-3 text-xs text-content overflow-x-auto whitespace-pre font-mono">
          {yaml}
        </pre>
      </div>

      {/* Setup instructions */}
      <div className="flex flex-col gap-2">
        <span className="text-xs font-medium text-content-secondary">Setup Instructions</span>
        <ol className="list-decimal list-inside flex flex-col gap-1.5 text-sm text-content-secondary">
          <li>Create <code className="text-xs bg-surface-raised px-1 py-0.5 rounded">.aida/</code> directory in your project root</li>
          <li>Save the file above as <code className="text-xs bg-surface-raised px-1 py-0.5 rounded">.aida/docker-compose.yml</code></li>
          <li>
            Initialize AIDA:
            <code className="block mt-1 text-xs bg-surface-raised px-2 py-1 rounded">
              docker compose -f .aida/docker-compose.yml run --rm aida aida init --no-skills --no-hooks
            </code>
          </li>
          <li>
            Start:
            <code className="block mt-1 text-xs bg-surface-raised px-2 py-1 rounded">
              docker compose -f .aida/docker-compose.yml up
            </code>
          </li>
          <li>
            Open <code className="text-xs bg-surface-raised px-1 py-0.5 rounded">http://localhost:{displayPort}</code>
          </li>
        </ol>
      </div>
    </div>
  );
}
