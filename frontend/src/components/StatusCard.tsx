interface StatusCardProps {
  title: string;
  status: string;
  details?: Array<{
    label: string;
    value: string | number;
  }>;
}

const StatusCard = ({ title, status, details = [] }: StatusCardProps) => {
  return (
    <div className="status-section">
      <div className="status-title">{title.toUpperCase()}</div>
      <div className="status-value">
        <span className={`status-text ${status.toLowerCase()}`}>
          {status.toUpperCase()}
        </span>
        {details.length > 0 && (
          <dl className="status-details">
            {details.map(detail => (
              <div className="status-detail-row" key={detail.label}>
                <dt>{detail.label}</dt>
                <dd>{detail.value}</dd>
              </div>
            ))}
          </dl>
        )}
      </div>
    </div>
  );
};

export default StatusCard;
