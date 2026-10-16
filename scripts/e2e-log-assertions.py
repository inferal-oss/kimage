"""Bounded native build-log acceptance without echoing private build output."""
import json

FINAL_LINE = 'kimage-diagnostic-output-4999'
TRUNCATED = '[earlier output truncated]'
PHASES = {'Capturing','Captured','Submitted','Completed','Interrupted','Unavailable'} | {
    'Failed:'+reason for reason in ['ContainerfileSolveFailed','BuildProgressLineLimit',
    'BuildProgressLimit','BuildInputOrEngineFailed','OperationCancelled','BuilderClientUnavailable',
    'BuilderProgressUnavailable','BuildExportTooLarge','UnsupportedWorkerPlatform']}
REASONS = {'Backoff','BuildClientStopped','BuildDeadlineExceeded','BuildExportDeadline',
    'BuildExportTooLarge','BuildExportTransferFailed','BuildExportUnavailable','BuildFailed',
    'BuildPermissionOrAdmissionDenied','BuildProgressLimit','BuildProgressLineLimit',
    'BuildSessionRecordMissing','BuildUnavailable','BuilderBusy','BuilderUnavailable',
    'BuilderWorkerChanged','CancellationPending','Captured','Capturing','Completed',
    'ContainerfileSolveFailed','Executing','Failed','InputChangedDuringBuild',
    'OperationCancelled','Published','StaleOperation','Submitted'}


def image_summary(images):
    """Only allowlisted public status codes, never messages or arbitrary values."""
    result = []
    for image in images[:8]:
        status = image.get('status') or {}
        phase = (status.get('operation') or {}).get('phase')
        result.append(dict(phase=phase if phase in REASONS else 'unexpected',
            reasons=[c.get('reason') if c.get('reason') in REASONS else 'unexpected'
                     for c in status.get('conditions', [])[:8]]))
    return json.dumps(result, sort_keys=True)


def summary(live, archived, retained):
    def output(value):
        return dict(bytes=len(value.encode()), finalLine=FINAL_LINE in value,
                    truncated=TRUNCATED in value, upstreamClipped='output clipped' in value)
    phase = retained[0].get('phase') if len(retained) == 1 else None
    return json.dumps(dict(live=output(live), archived=output(archived),
                           retainedCount=len(retained),
                           phase=phase if phase in PHASES else 'unexpected'),
                      sort_keys=True)


def verify(live, archived, retained, attempt):
    diagnostic = summary(live, archived, retained)
    if not (len(retained) == 1 and retained[0]['attempt'] == attempt
            and retained[0]['truncated'] and retained[0]['phase'] == 'Failed:ContainerfileSolveFailed'):
        raise RuntimeError('Incorrect retained failure receipt: ' + diagnostic)
    # stdout and stderr are drained independently. The stderr exit marker may
    # arrive before the stdout burst and legitimately fall outside its tail.
    # FIFO within stdout guarantees its final line follows the whole burst.
    if FINAL_LINE not in live:
        raise RuntimeError('Live native failure output is missing: ' + diagnostic)
    if not (TRUNCATED in archived and FINAL_LINE in archived and len(archived.encode()) < 66000):
        raise RuntimeError('Invalid bounded retained output: ' + diagnostic)
