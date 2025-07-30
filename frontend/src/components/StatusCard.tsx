interface StatusCardProps {
  title: string;
  status: string;
}

const StatusCard = ({ title, status }: StatusCardProps) => {
  return (
    <div className="status-section">
      <div className="status-title">{title.toUpperCase()}</div>
      <div className="status-value">
        <span className={`status-text ${status.toLowerCase()}`}>
          {status.toUpperCase()}
        </span>
      </div>
    </div>
  );
};

export default StatusCard;
