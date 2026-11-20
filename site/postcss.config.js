const babelConfig = require("./babel.config");

module.exports = {
  plugins: {
    "@stylexjs/postcss-plugin": {
      // The site's own files, and those of `packages/ui`, where the tokens
      // and the components it shares with the studio are written.
      include: ["src/**/*.{js,jsx,ts,tsx}", "../packages/ui/src/**/*.{ts,tsx}"],
      babelConfig: {
        babelrc: false,
        parserOpts: { plugins: ["typescript", "jsx"] },
        plugins: babelConfig.plugins,
      },
      useCSSLayers: true,
    },
  },
};
