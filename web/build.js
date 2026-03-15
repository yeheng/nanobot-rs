import { build } from 'vite';

async function runBuild() {
    try {
        await build({
            root: process.cwd()
        });
        console.log('Build completed successfully.');
    } catch (e) {
        console.error('Build failed', e);
    }
}

runBuild();
