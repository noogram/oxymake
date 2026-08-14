cwlVersion: v1.2
class: CommandLineTool
baseCommand: sh
arguments:
  - -c
  - |
    printf 'immutable-cache-output\n' > result.txt
inputs: []
outputs:
  result:
    type: File
    outputBinding:
      glob: result.txt
