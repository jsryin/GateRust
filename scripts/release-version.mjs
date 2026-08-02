const prereleaseIdentifier = String.raw`(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)`;
const buildIdentifier = String.raw`[0-9A-Za-z-]+`;
const releaseTagPattern = new RegExp(
  String.raw`^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)` +
    String.raw`(?:-${prereleaseIdentifier}(?:\.${prereleaseIdentifier})*)?` +
    String.raw`(?:\+${buildIdentifier}(?:\.${buildIdentifier})*)?$`,
);

export const versionFromReleaseTag = (tag) => {
  if (!releaseTagPattern.test(tag ?? "")) {
    throw new Error(
      "TAG must be a SemVer release tag such as v1.2.3 or v1.2.3-beta.1",
    );
  }
  return tag.slice(1);
};
