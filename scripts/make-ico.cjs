const { spawnSync } = require('node:child_process');
const path = require('node:path');
const result = spawnSync('python', [path.join(__dirname, 'build-icons.py')], { stdio: 'inherit' });
process.exit(result.status ?? 1);
